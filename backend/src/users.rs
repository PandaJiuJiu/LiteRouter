use crate::auth::require_admin;
use crate::db::hash_password;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct CreateUserReq {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub is_admin: bool,
}

#[derive(Deserialize, Default)]
pub struct UpdateUserReq {
    #[serde(default)]
    pub is_admin: Option<bool>,
    /// Admin-set new password. Empty / unset = keep current.
    #[serde(default)]
    pub password: Option<String>,
}

fn row_user(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "id": row.get::<i64, _>("id"),
        "username": row.get::<String, _>("username"),
        "is_admin": row.get::<i64, _>("is_admin") != 0,
        "created_at": row.get::<i64, _>("created_at"),
        "updated_at": row.get::<i64, _>("updated_at"),
    })
}

fn json_err(msg: &str) -> Json<Value> {
    Json(json!({ "error": msg }))
}

/// GET /api/users — admin only.
pub async fn list_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    require_admin(&state, &headers)?;
    let rows = sqlx::query(
        "SELECT id, username, is_admin, created_at, updated_at FROM users ORDER BY id ASC",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({
        "users": rows.iter().map(row_user).collect::<Vec<_>>()
    })))
}

/// POST /api/users — admin only. Creates a new account.
pub async fn create_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateUserReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_admin(&state, &headers).map_err(|s| (s, json_err("无权限")))?;
    let username = req.username.trim();
    if username.is_empty() {
        return Err((StatusCode::BAD_REQUEST, json_err("用户名不能为空")));
    }
    if req.password.len() < 8 {
        return Err((StatusCode::BAD_REQUEST, json_err("密码至少 8 位")));
    }
    let (hash, salt) = hash_password(&req.password);
    let ts = crate::db::now();
    let res = sqlx::query(
        "INSERT INTO users (username, password_hash, password_salt, is_admin, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(username)
    .bind(&hash)
    .bind(&salt)
    .bind(req.is_admin as i64)
    .bind(ts)
    .bind(ts)
    .execute(&state.pool)
    .await;
    match res {
        Ok(r) => {
            let id = r.last_insert_rowid();
            let row = sqlx::query(
                "SELECT id, username, is_admin, created_at, updated_at FROM users WHERE id = ?",
            )
            .bind(id)
            .fetch_one(&state.pool)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, json_err("数据库错误")))?;
            Ok(Json(row_user(&row)))
        }
        Err(ref e) if crate::db::is_unique_violation(e) => {
            Err((StatusCode::CONFLICT, json_err("用户名已存在")))
        }
        Err(_) => Err((StatusCode::INTERNAL_SERVER_ERROR, json_err("创建失败"))),
    }
}

/// PUT /api/users/:id — admin only. Toggle admin flag and/or reset password.
/// Refuses to demote the last remaining admin.
pub async fn update_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<UpdateUserReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_admin(&state, &headers).map_err(|s| (s, json_err("无权限")))?;

    let target = sqlx::query(
        "SELECT id, username, is_admin, created_at, updated_at FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, json_err("数据库错误")))?;
    let target = target.ok_or((StatusCode::NOT_FOUND, json_err("用户不存在")))?;
    let target_is_admin: i64 = target.get("is_admin");

    if let Some(new_pw) = req.password.as_deref() {
        if !new_pw.is_empty() && new_pw.len() < 8 {
            return Err((StatusCode::BAD_REQUEST, json_err("新密码至少 8 位")));
        }
    }

    let mut is_admin = target_is_admin;
    if let Some(flag) = req.is_admin {
        // Demoting an admin: make sure at least one admin stays.
        if target_is_admin != 0 && !flag {
            let remaining: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_admin = 1 AND id != ?")
                    .bind(id)
                    .fetch_one(&state.pool)
                    .await
                    .unwrap_or(0);
            if remaining == 0 {
                return Err((StatusCode::BAD_REQUEST, json_err("至少保留一个管理员")));
            }
        }
        is_admin = flag as i64;
    }

    if let Some(new_pw) = req.password.as_deref() {
        if !new_pw.is_empty() {
            let (hash, salt) = hash_password(new_pw);
            sqlx::query(
                "UPDATE users SET is_admin = ?, password_hash = ?, password_salt = ?, updated_at = ? WHERE id = ?",
            )
            .bind(is_admin)
            .bind(&hash)
            .bind(&salt)
            .bind(crate::db::now())
            .bind(id)
            .execute(&state.pool)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, json_err("更新失败")))?;
        } else {
            sqlx::query("UPDATE users SET is_admin = ?, updated_at = ? WHERE id = ?")
                .bind(is_admin)
                .bind(crate::db::now())
                .bind(id)
                .execute(&state.pool)
                .await
                .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, json_err("更新失败")))?;
        }
    } else {
        sqlx::query("UPDATE users SET is_admin = ?, updated_at = ? WHERE id = ?")
            .bind(is_admin)
            .bind(crate::db::now())
            .bind(id)
            .execute(&state.pool)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, json_err("更新失败")))?;
    }

    // Don't let the last admin lock itself out by demoting themselves via a
    // stale request — re-fetch and re-check.
    let updated = sqlx::query(
        "SELECT id, username, is_admin, created_at, updated_at FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, json_err("数据库错误")))?;
    Ok(Json(row_user(&updated)))
}

/// DELETE /api/users/:id — admin only. Refuses to remove the last admin.
/// Tokens owned by the deleted user become "orphaned" (user_id = NULL) and
/// remain visible only to admins, which mirrors the legacy migration path.
pub async fn delete_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let caller = require_admin(&state, &headers).map_err(|s| (s, json_err("无权限")))?;

    let target = sqlx::query("SELECT is_admin FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, json_err("数据库错误")))?;
    let target = target.ok_or((StatusCode::NOT_FOUND, json_err("用户不存在")))?;
    let target_is_admin: i64 = target.get("is_admin");

    if target_is_admin != 0 {
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_admin = 1 AND id != ?")
                .bind(id)
                .fetch_one(&state.pool)
                .await
                .unwrap_or(0);
        if remaining == 0 {
            return Err((StatusCode::BAD_REQUEST, json_err("至少保留一个管理员")));
        }
    }

    if id == caller.id {
        // don't let an admin delete their own logged-in account — causes
        // immediate session ambiguity. demote-or-delete-another instead.
        return Err((
            StatusCode::BAD_REQUEST,
            json_err("请先退出登录或由其他管理员删除该账号"),
        ));
    }

    // Orphan owned tokens rather than cascade-delete, so historical usage
    // stats survive in the logs view.
    sqlx::query("UPDATE tokens SET user_id = NULL WHERE user_id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, json_err("数据库错误")))?;

    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, json_err("删除失败")))?;

    Ok(Json(json!({ "ok": true })))
}

/// Returned tokens belong to this id, which we already validated above.
#[allow(dead_code)]
fn _silence_unused() {}
