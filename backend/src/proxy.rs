//! OpenAI-compatible relay: auth with internal token, route model -> channel,
//! forward request (streaming included) to the upstream channel.
//!
//! Failover: when a model is provided by more than one enabled channel, the
//! relay tries them in order and falls over to the next candidate on transport
//! errors or retriable upstream statuses (5xx, 408, 429, 524). Non-retriable
//! upstream statuses (4xx other than 408/429) are returned immediately so we
//! don't burn quota on a misrouted / malformed request.

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

/// Validate internal token and enforce per-token quotas. Returns token name.
/// Errors carry a status + user-facing message:
///   (UNAUTHORIZED, "invalid or disabled token") — bad/disabled key
///   (TOO_MANY_REQUESTS, msg) — rpm or daily-token limit exceeded
async fn auth_token(state: &AppState, key: &str) -> Result<String, (StatusCode, &'static str)> {
    let row = sqlx::query("SELECT name, enabled, rpm_limit, daily_token_limit FROM tokens WHERE key=?")
        .bind(key)
        .fetch_optional(&state.pool)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "db error"))?
        .ok_or((StatusCode::UNAUTHORIZED, "invalid or disabled token"))?;
    if row.get::<i64, _>("enabled") != 1 {
        return Err((StatusCode::UNAUTHORIZED, "invalid or disabled token"));
    }
    let name = row.get::<String, _>("name");
    let rpm_limit: i64 = row.get::<i64, _>("rpm_limit");
    let daily_limit: i64 = row.get::<i64, _>("daily_token_limit");

    // rpm check: count requests in last 60s
    if rpm_limit > 0 {
        let since = now() - 60;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM logs WHERE token_name=? AND created_at >= ?",
        )
        .bind(&name)
        .bind(since)
        .fetch_one(&state.pool)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "db error"))?;
        if count >= rpm_limit {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                "rate limit exceeded (rpm)",
            ));
        }
    }

    // daily token check: sum total_tokens since start of UTC day
    if daily_limit > 0 {
        let since_day = (now() / 86400) * 86400;
        let used: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(total_tokens), 0) FROM logs WHERE token_name=? AND created_at >= ?",
        )
        .bind(&name)
        .bind(since_day)
        .fetch_one(&state.pool)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "db error"))?;
        if used >= daily_limit {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                "daily token quota exceeded",
            ));
        }
    }

    let _ = sqlx::query("UPDATE tokens SET accessed_at=? WHERE key=?")
        .bind(now())
        .bind(key)
        .execute(&state.pool)
        .await;
    Ok(name)
}

/// All enabled channels that claim to serve `model` on `protocol`. Returned in
/// DB order (caller can override by stable sort, but row id is monotonic so the
/// "first registered channel wins" rule is preserved).
async fn candidate_channels(
    state: &AppState,
    model: &str,
    protocol: &str,
) -> Result<Vec<(String, String, String)>, StatusCode> {
    let rows = sqlx::query(
        "SELECT name, base_url, base_url_anthropic, api_key, models FROM channels WHERE enabled=1",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut out = Vec::new();
    for row in rows {
        let models_str: String = row.get::<String, _>("models");
        let matches = models_str
            .split(',')
            .map(|s| s.trim())
            .any(|m| m == model || m == "*");
        if !matches {
            continue;
        }
        let base_url = match protocol {
            "anthropic" => row.get::<String, _>("base_url_anthropic"),
            _ => row.get::<String, _>("base_url"),
        };
        if base_url.is_empty() {
            continue; // this channel doesn't serve the requested protocol
        }
        out.push((
            row.get::<String, _>("name"),
            base_url,
            row.get::<String, _>("api_key"),
        ));
    }
    Ok(out)
}

async fn log_request(
    state: &AppState,
    token_name: &str,
    model: &str,
    channel_name: &str,
    status: i64,
    usage: Option<(i64, i64, i64)>,
) {
    let (p, c, t) = usage.unwrap_or((0, 0, 0));
    let _ = sqlx::query(
        "INSERT INTO logs (token_name, model, channel_name, status_code, prompt_tokens, completion_tokens, total_tokens, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(token_name)
    .bind(model)
    .bind(channel_name)
    .bind(status)
    .bind(p)
    .bind(c)
    .bind(t)
    .bind(now())
    .execute(&state.pool)
    .await;
}

/// Extract (prompt, completion, total) token counts from an upstream JSON body.
/// Supports OpenAI ({prompt_tokens, completion_tokens, total_tokens}) and
/// Anthropic ({input_tokens, output_tokens}) shapes.
fn parse_usage(body: &[u8]) -> Option<(i64, i64, i64)> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let u = v.get("usage")?;
    let prompt = u.get("prompt_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
    let completion = u.get("completion_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
    // Anthropic uses input_tokens/output_tokens
    let input = u.get("input_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
    let output = u.get("output_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
    let p = if prompt > 0 { prompt } else { input };
    let c = if completion > 0 { completion } else { output };
    if p == 0 && c == 0 {
        return None;
    }
    let t = u.get("total_tokens").and_then(|x| x.as_i64()).unwrap_or(p + c);
    Some((p, c, t))
}

fn error_response(status: StatusCode, message: &str) -> Response {
    let body = json!({
        "error": { "message": message, "type": "lite_one_api_error" }
    });
    (status, Json(body)).into_response()
}

/// Status codes that signal "this upstream is having a bad time, try the next
/// channel". 4xx other than these are the client's fault — no point failing
/// over to a second provider only to get the same 400.
fn is_retriable_status(code: u16) -> bool {
    code == 408 || code == 429 || code >= 500
}

/// Build the upstream request with auth + protocol-appropriate headers. The
/// caller still owns the body, so this is just a header recipe.
fn build_request(
    state: &AppState,
    base_url: &str,
    api_key: &str,
    protocol: &str,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    let req = if protocol == "anthropic" {
        let version = headers
            .get("anthropic-version")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("2023-06-01");
        state
            .http
            .post(format!("{}/v1/messages", base_url))
            .header("x-api-key", api_key)
            .header("anthropic-version", version)
    } else {
        state
            .http
            .post(format!("{}/chat/completions", base_url))
            .header("Authorization", format!("Bearer {}", api_key))
    };
    req.header("Content-Type", "application/json")
}

/// Convert a successful upstream response into the client response, logging
/// the outcome against `channel_name`. Streaming requests pass through
/// unchanged; non-streaming responses are buffered so we can extract usage.
async fn respond_from_upstream(
    state: &AppState,
    token_name: &str,
    model: &str,
    channel_name: &str,
    resp: reqwest::Response,
    is_streaming: bool,
) -> Response {
    let status =
        StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

    if !is_streaming {
        let content_type = resp.headers().get("content-type").cloned();
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => {
                log_request(
                    state,
                    token_name,
                    model,
                    channel_name,
                    status.as_u16() as i64,
                    None,
                )
                .await;
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    &format!("upstream body read failed: {}", e),
                );
            }
        };
        let usage = parse_usage(&bytes);
        log_request(
            state,
            token_name,
            model,
            channel_name,
            status.as_u16() as i64,
            usage,
        )
        .await;
        let mut builder = Response::builder().status(status);
        if let Some(ct) = content_type {
            if let Some(h) = builder.headers_mut() {
                h.insert(axum::http::header::CONTENT_TYPE, ct);
            }
        }
        return builder
            .body(Body::from(bytes))
            .unwrap_or_else(|_| error_response(StatusCode::BAD_GATEWAY, "body build failed"));
    }

    // Streaming: pass upstream through, log status only.
    log_request(
        state,
        token_name,
        model,
        channel_name,
        status.as_u16() as i64,
        None,
    )
    .await;
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

/// Common relay: auth -> route -> forward with multi-channel failover.
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
        Err((s, msg)) => return error_response(s, msg),
    };

    // 2. parse body to find model + streaming flag
    let req_json: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid JSON body"),
    };
    let model = match req_json.get("model").and_then(|m| m.as_str()) {
        Some(m) => m.to_string(),
        None => return error_response(StatusCode::BAD_REQUEST, "missing model in request"),
    };
    let is_streaming = req_json
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // 3. collect all candidate channels for this model+protocol
    let candidates = match candidate_channels(state, &model, protocol).await {
        Ok(c) => c,
        Err(s) => return error_response(s, "internal error"),
    };
    if candidates.is_empty() {
        return error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "no enabled channel provides model `{}` on the {} protocol",
                model, protocol
            ),
        );
    }

    // 4. try each candidate; failover on transport errors and retriable status.
    let mut last_err = String::new();
    for (channel_name, base_url, api_key) in &candidates {
        let req = build_request(state, base_url, api_key, protocol, headers)
            .body(body.to_vec());
        match req.send().await {
            Ok(resp) => {
                let code = resp.status().as_u16();
                if resp.status().is_success() {
                    return respond_from_upstream(
                        state,
                        &token_name,
                        &model,
                        channel_name,
                        resp,
                        is_streaming,
                    )
                    .await;
                }
                if !is_retriable_status(code) {
                    // non-retriable: surface upstream's response as-is
                    return respond_from_upstream(
                        state,
                        &token_name,
                        &model,
                        channel_name,
                        resp,
                        is_streaming,
                    )
                    .await;
                }
                // retriable: log + try next
                log_request(state, &token_name, &model, channel_name, code as i64, None).await;
                last_err = format!("{} -> HTTP {}", channel_name, code);
            }
            Err(e) => {
                log_request(state, &token_name, &model, channel_name, -1, None).await;
                last_err = format!("{}: {}", channel_name, e);
            }
        }
    }

    error_response(
        StatusCode::BAD_GATEWAY,
        &format!("all {} candidate(s) failed: {}", candidates.len(), last_err),
    )
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
    if let Err((s, msg)) = auth_token(&state, &key).await {
        return error_response(s, msg);
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