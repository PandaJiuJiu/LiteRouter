//! OpenAI-compatible relay: auth with internal token, route model -> channel,
//! forward request (streaming included) to the upstream channel.

use crate::db::now;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use sqlx::Row;
use std::sync::Arc;

fn extract_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

/// Validate internal token, update accessed_at. Returns token name.
async fn auth_token(state: &AppState, key: &str) -> Result<String, StatusCode> {
    let row = sqlx::query("SELECT name, enabled FROM tokens WHERE key=?")
        .bind(key)
        .fetch_optional(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if row.get::<i64, _>("enabled") != 1 {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let name = row.get::<String, _>("name");
    let _ = sqlx::query("UPDATE tokens SET accessed_at=? WHERE key=?")
        .bind(now())
        .bind(key)
        .execute(&state.pool)
        .await;
    Ok(name)
}

/// Pick an enabled channel whose models list contains `model`.
async fn find_channel(
    state: &AppState,
    model: &str,
) -> Result<(String, String, String), StatusCode> {
    let rows = sqlx::query("SELECT name, base_url, api_key, models FROM channels WHERE enabled=1")
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    for row in rows {
        let models_str: String = row.get::<String, _>("models");
        let models: Vec<&str> = models_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if models.iter().any(|m| *m == model) {
            return Ok((
                row.get::<String, _>("name"),
                row.get::<String, _>("base_url"),
                row.get::<String, _>("api_key"),
            ));
        }
    }
    Err(StatusCode::NOT_FOUND)
}

async fn log_request(
    state: &AppState,
    token_name: &str,
    model: &str,
    channel_name: &str,
    status: i64,
) {
    let _ = sqlx::query(
        "INSERT INTO logs (token_name, model, channel_name, status_code, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(token_name)
    .bind(model)
    .bind(channel_name)
    .bind(status)
    .bind(now())
    .execute(&state.pool)
    .await;
}

fn error_response(status: StatusCode, message: &str) -> Response {
    let body = json!({
        "error": { "message": message, "type": "lite_one_api_error" }
    });
    (status, Json(body)).into_response()
}

/// POST /v1/chat/completions — relay to upstream channel, pass stream through.
pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // 1. internal token auth
    let key = match extract_token(&headers) {
        Some(k) => k,
        None => return error_response(StatusCode::UNAUTHORIZED, "missing bearer token"),
    };
    let token_name = match auth_token(&state, &key).await {
        Ok(n) => n,
        Err(s) => return error_response(s, "invalid or disabled token"),
    };

    // 2. parse body to find model
    let req_json: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid JSON body"),
    };
    let model = match req_json.get("model").and_then(|m| m.as_str()) {
        Some(m) => m.to_string(),
        None => return error_response(StatusCode::BAD_REQUEST, "missing model in request"),
    };

    // 3. route to channel
    let (channel_name, base_url, api_key) = match find_channel(&state, &model).await {
        Ok(c) => c,
        Err(StatusCode::NOT_FOUND) => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("no enabled channel provides model `{}`", model),
            )
        }
        Err(s) => return error_response(s, "internal error"),
    };

    // 4. forward
    let url = format!("{}/v1/chat/completions", base_url);
    let resp = state
        .http
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .body(body.to_vec())
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            log_request(&state, &token_name, &model, &channel_name, -1).await;
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("upstream request failed: {}", e),
            );
        }
    };

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    log_request(&state, &token_name, &model, &channel_name, status.as_u16() as i64).await;

    // 5. pass response through — reqwest stream directly into axum body so
    //    SSE streaming works without buffering.
    let content_type = resp.headers().get("content-type").cloned();
    let stream = resp.bytes_stream();
    let mut response = Response::builder().status(status);
    if let Some(ct) = content_type {
        if let Some(h) = response.headers_mut() {
            h.insert(axum::http::header::CONTENT_TYPE, ct);
        }
    }
    response
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| error_response(StatusCode::BAD_GATEWAY, "body build failed"))
}

/// GET /v1/models — list union of all enabled channel models.
pub async fn list_models(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let key = match extract_token(&headers) {
        Some(k) => k,
        None => return error_response(StatusCode::UNAUTHORIZED, "missing bearer token"),
    };
    if let Err(s) = auth_token(&state, &key).await {
        return error_response(s, "invalid or disabled token");
    }
    let rows = match sqlx::query("SELECT models FROM channels WHERE enabled=1")
        .fetch_all(&state.pool)
        .await
    {
        Ok(r) => r,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "db error"),
    };
    let mut models: Vec<String> = Vec::new();
    for row in rows {
        for m in row.get::<String, _>("models").split(',') {
            let m = m.trim();
            if !m.is_empty() && !models.iter().any(|x| x == m) {
                models.push(m.to_string());
            }
        }
    }
    let data: Vec<Value> = models
        .iter()
        .map(|m| json!({ "id": m, "object": "model", "owned_by": "lite-one-api" }))
        .collect();
    Json(json!({ "object": "list", "data": data })).into_response()
}
