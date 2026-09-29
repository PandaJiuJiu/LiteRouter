//! OpenAI-compatible relay: auth with internal token, route model -> channel,
//! forward request (streaming included) to the upstream channel.

use crate::db::now;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::StreamExt;
use serde_json::{json, Value};
use sqlx::Row;
use std::sync::Arc;

fn extract_token(headers: &HeaderMap) -> Option<String> {
    // OpenAI clients: "Authorization: Bearer sk-..."
    if let Some(t) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        return Some(t.to_string());
    }
    // Anthropic clients: "x-api-key: sk-..."
    headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
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
/// `protocol` selects which upstream base URL to use: "openai" or "anthropic".
async fn find_channel(
    state: &AppState,
    model: &str,
    protocol: &str,
) -> Result<(String, String, String), StatusCode> {
    let rows = sqlx::query(
        "SELECT name, base_url, base_url_anthropic, api_key, models FROM channels WHERE enabled=1",
    )
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
        // "*" is a wildcard: channel accepts any model
        if models.iter().any(|m| *m == model || *m == "*") {
            let base_url = match protocol {
                "anthropic" => row.get::<String, _>("base_url_anthropic"),
                _ => row.get::<String, _>("base_url"),
            };
            if base_url.is_empty() {
                continue; // this channel doesn't serve the requested protocol
            }
            return Ok((
                row.get::<String, _>("name"),
                base_url,
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

/// Common relay: auth -> route -> forward, streaming the upstream response back.
async fn relay(
    state: &AppState,
    headers: &HeaderMap,
    body: axum::body::Bytes,
    protocol: &str,
) -> Response {
    // 1. internal token auth
    let key = match extract_token(headers) {
        Some(k) => k,
        None => return error_response(StatusCode::UNAUTHORIZED, "missing bearer token"),
    };
    let token_name = match auth_token(state, &key).await {
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

    // 3. route to channel (protocol-aware)
    let (channel_name, base_url, api_key) = match find_channel(state, &model, protocol).await {
        Ok(c) => c,
        Err(StatusCode::NOT_FOUND) => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!(
                    "no enabled channel provides model `{}` on the {} protocol",
                    model, protocol
                ),
            )
        }
        Err(s) => return error_response(s, "internal error"),
    };

    // 4. forward
    let req = if protocol == "anthropic" {
        let version = headers
            .get("anthropic-version")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("2023-06-01");
        state
            .http
            .post(format!("{}/v1/messages", base_url))
            .header("x-api-key", &api_key)
            .header("anthropic-version", version)
    } else {
        state
            .http
            .post(format!("{}/chat/completions", base_url))
            .header("Authorization", format!("Bearer {}", api_key))
    };
    let resp = req
        .header("Content-Type", "application/json")
        .body(body.to_vec())
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            log_request(state, &token_name, &model, &channel_name, -1).await;
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("upstream request failed: {}", e),
            );
        }
    };

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    eprintln!(
        "[relay] upstream {} -> {} ct={:?} len={:?}",
        resp.status(),
        status,
        resp.headers().get("content-type").map(|v| v.to_str().ok()),
        resp.headers().get("content-length")
    );
    log_request(state, &token_name, &model, &channel_name, status.as_u16() as i64).await;

    // 5. pass response through — reqwest stream directly into axum body so
    //    SSE streaming works without buffering.
    let content_type = resp.headers().get("content-type").cloned();
    let stream = resp
        .bytes_stream()
        .inspect(|chunk| {
            if let Ok(b) = chunk {
                eprintln!("[relay] chunk {} bytes", b.len());
            } else {
                eprintln!("[relay] stream error: {:?}", chunk);
            }
        });
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

/// POST /v1/chat/completions — OpenAI-compatible relay.
pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    relay(&state, &headers, body, "openai").await
}

/// POST /v1/messages — Anthropic Messages API relay.
pub async fn anthropic_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    relay(&state, &headers, body, "anthropic").await
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
            if m.is_empty() || m == "*" {
                continue; // wildcard channels don't enumerate models
            }
            if !models.iter().any(|x| x == m) {
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
