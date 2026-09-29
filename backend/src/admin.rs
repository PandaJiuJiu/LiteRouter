use crate::auth::check_admin;
use crate::db::now;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use std::sync::Arc;

fn row_channel(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "id": row.get::<i64, _>("id"),
        "name": row.get::<String, _>("name"),
        "base_url": row.get::<String, _>("base_url"),
        "api_key": row.get::<String, _>("api_key"),
        "models": row.get::<String, _>("models"),
        "enabled": row.get::<i64, _>("enabled"),
        "created_at": row.get::<i64, _>("created_at"),
    })
}

fn row_token(row: &sqlx::sqlite::SqliteRow, show_key: bool) -> Value {
    let mut v = json!({
        "id": row.get::<i64, _>("id"),
        "name": row.get::<String, _>("name"),
        "enabled": row.get::<i64, _>("enabled"),
        "created_at": row.get::<i64, _>("created_at"),
        "accessed_at": row.get::<i64, _>("accessed_at"),
    });
    if show_key {
        v["key"] = json!(row.get::<String, _>("key"));
    }
    v
}

// ---------- channels ----------

#[derive(Deserialize)]
pub struct ChannelReq {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub models: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}
fn default_true() -> bool {
    true
}

pub async fn list_channels(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    let rows = sqlx::query("SELECT * FROM channels ORDER BY id DESC")
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({
        "channels": rows.iter().map(row_channel).collect::<Vec<_>>()
    })))
}

pub async fn create_channel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ChannelReq>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    sqlx::query("INSERT INTO channels (name, base_url, api_key, models, enabled, created_at) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(&req.name)
        .bind(req.base_url.trim_end_matches('/'))
        .bind(&req.api_key)
        .bind(&req.models)
        .bind(req.enabled as i64)
        .bind(now())
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn update_channel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<ChannelReq>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    sqlx::query("UPDATE channels SET name=?, base_url=?, api_key=?, models=?, enabled=? WHERE id=?")
        .bind(&req.name)
        .bind(req.base_url.trim_end_matches('/'))
        .bind(&req.api_key)
        .bind(&req.models)
        .bind(req.enabled as i64)
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_channel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    sqlx::query("DELETE FROM channels WHERE id=?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "ok": true })))
}

// ---------- tokens ----------

#[derive(Deserialize)]
pub struct TokenReq {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

pub async fn list_tokens(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    let rows = sqlx::query("SELECT * FROM tokens ORDER BY id DESC")
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({
        "tokens": rows.iter().map(|r| row_token(r, true)).collect::<Vec<_>>()
    })))
}

pub async fn create_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<TokenReq>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    let key = format!("sk-{}", uuid::Uuid::new_v4().simple());
    sqlx::query("INSERT INTO tokens (name, key, enabled, created_at) VALUES (?, ?, ?, ?)")
        .bind(&req.name)
        .bind(&key)
        .bind(req.enabled as i64)
        .bind(now())
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let row = sqlx::query("SELECT * FROM tokens WHERE key=?")
        .bind(&key)
        .fetch_one(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(row_token(&row, true)))
}

pub async fn toggle_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    let enabled = body
        .get("enabled")
        .and_then(|v| v.as_bool())
        .ok_or(StatusCode::BAD_REQUEST)? as i64;
    sqlx::query("UPDATE tokens SET enabled=? WHERE id=?")
        .bind(enabled)
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    sqlx::query("DELETE FROM tokens WHERE id=?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "ok": true })))
}

// ---------- logs ----------

#[derive(Deserialize)]
pub struct PageQuery {
    #[serde(default = "default_page")]
    pub page: i64,
    #[serde(default = "default_size")]
    pub size: i64,
}
fn default_page() -> i64 {
    1
}
fn default_size() -> i64 {
    50
}

pub async fn list_logs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    let offset = (q.page - 1).max(0) * q.size;
    let rows = sqlx::query("SELECT * FROM logs ORDER BY id DESC LIMIT ? OFFSET ?")
        .bind(q.size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let logs: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.get::<i64, _>("id"),
                "token_name": r.get::<String, _>("token_name"),
                "model": r.get::<String, _>("model"),
                "channel_name": r.get::<String, _>("channel_name"),
                "status_code": r.get::<i64, _>("status_code"),
                "created_at": r.get::<i64, _>("created_at"),
            })
        })
        .collect();
    Ok(Json(json!({ "logs": logs })))
}
