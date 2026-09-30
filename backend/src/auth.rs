use crate::db;
use crate::db::{hash_password, verify_password};
use crate::state::{AppState, SessionInfo};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use std::sync::Arc;

/// Authenticated principal extracted from the session header. Returned by
/// `check_auth` so handlers can scope queries to the caller.
#[derive(Clone, Copy)]
pub struct AuthUser {
    pub id: i64,
    pub is_admin: bool,
}

#[derive(Deserialize)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct SetupReq {
    pub username: String,
    pub password: String,
    /// 首次初始化时选择的 UI 语言。`Option` 保持对旧前端的兼容，缺省落到中文。
    pub language: Option<String>,
}

#[derive(Deserialize)]
pub struct PasswordChangeReq {
    pub old_password: String,
    pub new_password: String,
}

/// Pull the bearer session token out of the Authorization header.
fn bearer_session(headers: &HeaderMap) -> Result<String, StatusCode> {
    headers
        .get("authorization")
        .and_then(|v: &HeaderValue| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string())
        .ok_or(StatusCode::UNAUTHORIZED)
}

/// GET /api/setup-status — public, lets the SPA decide whether to show the
/// setup wizard, login form, or nothing. Also tells the SPA whether an
/// existing session is still valid (avoids a second /api/me roundtrip on
/// every page load).
pub async fn setup_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Json<Value> {
    let user_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&state.pool)
            .await
            .unwrap_or(0);
    let auth = check_auth(&state, &headers).ok();
    let username = match auth {
        Some(u) => sqlx::query_scalar::<_, String>("SELECT username FROM users WHERE id = ?")
            .bind(u.id)
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    Json(json!({
        "needsSetup": user_count == 0,
        "authenticated": auth.is_some(),
        "is_admin": auth.map(|u| u.is_admin).unwrap_or(false),
        "username": username,
        "language": crate::settings::current_language(&state.pool).await,
    }))
}

/// POST /api/setup — only valid while users table is empty. Creates the
/// first admin and immediately issues a session.
pub async fn setup(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetupReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let username = req.username.trim();
    let password = req.password;
    if username.is_empty() {
        return Err((StatusCode::BAD_REQUEST, json_err("用户名不能为空")));
    }
    if password.len() < 8 {
        return Err((StatusCode::BAD_REQUEST, json_err("密码至少 8 位")));
    }

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&state.pool)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, json_err("数据库错误")))?;
    if count > 0 {
        return Err((StatusCode::FORBIDDEN, json_err("已初始化，禁止重复设置")));
    }

    let (hash, salt) = hash_password(&password);
    let ts = db::now();
    let res = sqlx::query(
        "INSERT INTO users (username, password_hash, password_salt, is_admin, created_at, updated_at)
         VALUES (?, ?, ?, 1, ?, ?)",
    )
    .bind(username)
    .bind(&hash)
    .bind(&salt)
    .bind(ts)
    .bind(ts)
    .execute(&state.pool)
    .await;
    let user_id = match res {
        Ok(r) => r.last_insert_rowid(),
        Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("20602") => {
            return Err((StatusCode::CONFLICT, json_err("用户名已存在")))
        }
        Err(_) => {
            return Err((StatusCode::INTERNAL_SERVER_ERROR, json_err("创建失败")))
        }
    };

    let session = uuid::Uuid::new_v4().to_string();
    state.sessions.lock().unwrap().insert(
        session.clone(),
        SessionInfo {
            user_id,
            is_admin: true,
            created_at: ts,
        },
    );

    // 语言在向导里就选好了，这里顺手落库 —— 免得前端再发一次请求。
    let language = crate::settings::normalize_language(req.language.as_deref());
    crate::settings::store_language(&state.pool, &language).await;

    Ok(Json(json!({
        "session": session,
        "username": username,
        "is_admin": true,
        "language": language,
    })))
}

/// POST /api/login — username + password, returns session.
pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let row = sqlx::query(
        "SELECT id, username, password_hash, password_salt, is_admin
         FROM users WHERE username = ?",
    )
    .bind(req.username.trim())
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, json_err("数据库错误")))?;
    let row = match row {
        Some(r) => r,
        None => return Err((StatusCode::UNAUTHORIZED, json_err("用户名或密码错误"))),
    };
    let hash: String = row.get("password_hash");
    let salt: String = row.get("password_salt");
    if !verify_password(&req.password, &hash, &salt) {
        return Err((StatusCode::UNAUTHORIZED, json_err("用户名或密码错误")));
    }
    let user_id: i64 = row.get("id");
    let is_admin: i64 = row.get("is_admin");
    let username: String = row.get("username");

    let session = uuid::Uuid::new_v4().to_string();
    state.sessions.lock().unwrap().insert(
        session.clone(),
        SessionInfo {
            user_id,
            is_admin: is_admin != 0,
            created_at: db::now(),
        },
    );
    Ok(Json(json!({
        "session": session,
        "username": username,
        "is_admin": is_admin != 0,
    })))
}

/// POST /api/logout — invalidate the current session.
pub async fn logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> StatusCode {
    if let Ok(session) = bearer_session(&headers) {
        state.sessions.lock().unwrap().remove(&session);
    }
    StatusCode::NO_CONTENT
}

/// GET /api/me — current user identity.
pub async fn me(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    let user = check_auth(&state, &headers)?;
    let row = sqlx::query("SELECT username, is_admin FROM users WHERE id = ?")
        .bind(user.id)
        .fetch_one(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({
        "id": user.id,
        "username": row.get::<String, _>("username"),
        "is_admin": row.get::<i64, _>("is_admin") != 0,
    })))
}

/// POST /api/password — change own password (requires current password).
pub async fn change_password(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<PasswordChangeReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let user = check_auth(&state, &headers).map_err(|s| (s, json_err("未登录")))?;
    if req.new_password.len() < 8 {
        return Err((StatusCode::BAD_REQUEST, json_err("新密码至少 8 位")));
    }
    let row = sqlx::query("SELECT password_hash, password_salt FROM users WHERE id = ?")
        .bind(user.id)
        .fetch_one(&state.pool)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, json_err("数据库错误")))?;
    if !verify_password(
        &req.old_password,
        &row.get::<String, _>("password_hash"),
        &row.get::<String, _>("password_salt"),
    ) {
        return Err((StatusCode::UNAUTHORIZED, json_err("当前密码错误")));
    }
    let (hash, salt) = hash_password(&req.new_password);
    sqlx::query(
        "UPDATE users SET password_hash = ?, password_salt = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&hash)
    .bind(&salt)
    .bind(db::now())
    .bind(user.id)
    .execute(&state.pool)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, json_err("更新失败")))?;
    Ok(Json(json!({ "ok": true })))
}

/// Validate the bearer token and return the calling user. Use this for any
/// handler that requires *some* logged-in principal. For admin-only handlers,
/// use `require_admin` instead.
pub fn check_auth(state: &AppState, headers: &HeaderMap) -> Result<AuthUser, StatusCode> {
    let session = bearer_session(headers)?;
    let info = state
        .sessions
        .lock()
        .unwrap()
        .get(&session)
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(AuthUser {
        id: info.user_id,
        is_admin: info.is_admin,
    })
}

/// Reject any caller that isn't an admin. Use for endpoints that should be
/// hidden from regular users (channel/mapping management, user CRUD).
pub fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<AuthUser, StatusCode> {
    let user = check_auth(state, headers)?;
    if !user.is_admin {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(user)
}

fn json_err(msg: &str) -> Json<Value> {
    Json(json!({ "error": msg }))
}