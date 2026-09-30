use crate::auth::require_admin;
use crate::db;
use crate::proxy;
use crate::state::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// GET /api/settings/debug-logging — whether request/response bodies are
/// being written to disk for debugging.
pub async fn get_debug_logging(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    let _user = require_admin(&state, &headers)?;

    let enabled = db::get_setting(&state.pool, "debug_logging")
        .await
        .ok()
        .flatten()
        .map(|v| v == "1")
        .unwrap_or(false);

    Ok(Json(json!({ "enabled": enabled })))
}

#[derive(Deserialize)]
pub struct SetDebugLoggingReq {
    enabled: bool,
}

/// PUT /api/settings/debug-logging — toggle debug body logging. Updates both
/// the DB and the in-memory flag so the change is effective immediately.
pub async fn set_debug_logging(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SetDebugLoggingReq>,
) -> Result<Json<Value>, StatusCode> {
    let _user = require_admin(&state, &headers)?;

    let value = if req.enabled { "1" } else { "0" };
    let _ = sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('debug_logging', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(value)
    .execute(&state.pool)
    .await;

    proxy::set_debug_logging(req.enabled);

    Ok(Json(json!({ "enabled": req.enabled })))
}
