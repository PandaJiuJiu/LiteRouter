use crate::state::AppState;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct LoginReq {
    pub password: String,
}

/// POST /api/login — admin password login, returns a session token.
pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginReq>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if req.password != AppState::admin_password() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let session = uuid::Uuid::new_v4().to_string();
    state
        .sessions
        .lock()
        .unwrap()
        .insert(session.clone(), crate::db::now());
    Ok(Json(json!({ "session": session })))
}

/// Check the admin session header. Err(status) means reject.
pub fn check_admin(state: &AppState, headers: &axum::http::HeaderMap) -> Result<(), StatusCode> {
    let session = headers
        .get("authorization")
        .and_then(|v: &HeaderValue| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    state
        .sessions
        .lock()
        .unwrap()
        .contains_key(session)
        .then_some(())
        .ok_or(StatusCode::UNAUTHORIZED)
}
