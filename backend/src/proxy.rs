//! OpenAI-compatible relay: auth with internal token, route model -> channel,
//! forward request (streaming included) to the upstream channel. All incoming
//! requests are internal and are routed to external channels only
//! (channels.kind='internal' never serves relay traffic).
//!
//! Failover: when a model is provided by more than one enabled channel, the
//! relay tries them in order and falls over to the next candidate on transport
//! errors or retriable upstream statuses (5xx, 408, 429, 524). Non-retriable
//! upstream statuses (4xx other than 408/429) are returned immediately so we
//! don't burn quota on a misrouted / malformed request.

use crate::admin;
use crate::convert::{self, ConvertMode, SseConverter};
use crate::db::now;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use futures_util::stream;
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

/// One routable upstream hop: the URL/key to hit and, when the channel only
/// speaks the other protocol, the conversion the relay must apply.
struct Candidate {
    name: String,
    base_url: String,
    api_key: String,
    convert: ConvertMode,
}

/// All enabled *external* channels that claim to serve `model` on `protocol`.
/// Every request reaching this relay is an internal request, and internal
/// requests are routed to external channels only — 'internal' channels never
/// serve relay traffic. Channels without a URL for the client's protocol but
/// with one for the other protocol are included with a conversion mode.
/// Returned in DB order (caller can override by stable sort, but row id is
/// monotonic so the "first registered channel wins" rule is preserved).
async fn candidate_channels(
    state: &AppState,
    model: &str,
    protocol: &str,
) -> Result<Vec<Candidate>, StatusCode> {
    let rows = sqlx::query(
        "SELECT name, base_url, base_url_anthropic, api_key, models FROM channels WHERE enabled=1 AND kind='external'",
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
        let openai_url: String = row.get("base_url");
        let anthropic_url: String = row.get("base_url_anthropic");
        let (base_url, convert) = match protocol {
            "anthropic" => {
                if !anthropic_url.is_empty() {
                    (anthropic_url, ConvertMode::None)
                } else if !openai_url.is_empty() {
                    (openai_url, ConvertMode::ToOpenAI)
                } else {
                    continue; // channel serves neither protocol
                }
            }
            _ => {
                if !openai_url.is_empty() {
                    (openai_url, ConvertMode::None)
                } else if !anthropic_url.is_empty() {
                    (anthropic_url, ConvertMode::ToAnthropic)
                } else {
                    continue;
                }
            }
        };
        out.push(Candidate {
            name: row.get("name"),
            base_url,
            api_key: row.get("api_key"),
            convert,
        });
    }
    Ok(out)
}

/// If `alias` is configured, return its ordered (channel, model) target list
/// (tried in array order); otherwise return `[("", alias)]` unchanged. Single
/// level rewrite — `a → b → c` is treated as `a → b`. A `model == "*"` entry
/// pinned to a channel expands to every model that channel advertises.
async fn resolve_targets(state: &AppState, alias: &str) -> Vec<(String, String)> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT targets, target_model FROM model_mappings WHERE alias=?",
    )
    .bind(alias)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);
    let base = match row {
        Some((targets, fallback)) => admin::parse_targets(&targets, &fallback),
        None => Vec::new(),
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for (channel, model) in base {
        if model == "*" && !channel.is_empty() {
            // wildcard: every model on the pinned channel, in its list order
            if let Ok(rows) = sqlx::query(
                "SELECT models FROM channels WHERE name=? AND enabled=1 AND kind='external'",
            )
            .bind(&channel)
            .fetch_all(&state.pool)
            .await
            {
                let models: Vec<String> = rows
                    .iter()
                    .flat_map(|r| {
                        r.get::<String, _>("models")
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|m| !m.is_empty() && m != "*")
                            .collect::<Vec<_>>()
                    })
                    .collect();
                for m in models {
                    if !out.iter().any(|(c, x)| c == &channel && x == &m) {
                        out.push((channel.clone(), m));
                    }
                }
            }
        } else if !out.iter().any(|(c, m)| c == &channel && m == &model) {
            out.push((channel, model));
        }
    }
    if out.is_empty() {
        out.push((String::new(), alias.to_string()));
    }
    out
}

/// A target pinned to a named channel: the URL/key/conversion to reach it on
/// `protocol`, or None when the channel doesn't exist / is disabled / serves
/// neither protocol.
async fn pinned_channel(
    state: &AppState,
    name: &str,
    protocol: &str,
) -> Option<Candidate> {
    let row = sqlx::query(
        "SELECT name, base_url, base_url_anthropic, api_key FROM channels WHERE name=? AND enabled=1 AND kind='external'",
    )
    .bind(name)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()?;
    let openai_url: String = row.get("base_url");
    let anthropic_url: String = row.get("base_url_anthropic");
    let (base_url, convert) = if protocol == "anthropic" {
        if !anthropic_url.is_empty() {
            (anthropic_url, ConvertMode::None)
        } else if !openai_url.is_empty() {
            (openai_url, ConvertMode::ToOpenAI)
        } else {
            return None;
        }
    } else if !openai_url.is_empty() {
        (openai_url, ConvertMode::None)
    } else if !anthropic_url.is_empty() {
        (anthropic_url, ConvertMode::ToAnthropic)
    } else {
        return None;
    };
    Some(Candidate {
        name: row.get("name"),
        base_url,
        api_key: row.get("api_key"),
        convert,
    })
}

async fn log_request(
    state: &AppState,
    token_name: &str,
    request_model: &str,
    model: &str,
    channel_name: &str,
    status: i64,
    usage: Option<(i64, i64, i64)>,
) {
    let (p, c, t) = usage.unwrap_or((0, 0, 0));
    let _ = sqlx::query(
        "INSERT INTO logs (token_name, model, channel_name, status_code, prompt_tokens, completion_tokens, total_tokens, created_at, request_model) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(token_name)
    .bind(model)
    .bind(channel_name)
    .bind(status)
    .bind(p)
    .bind(c)
    .bind(t)
    .bind(now())
    .bind(request_model)
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

fn error_response(status: StatusCode, message: &str, retry_after: Option<&str>) -> Response {
    let body = json!({
        "error": { "message": message, "type": "literouter_error" }
    });
    let mut resp = (status, Json(body)).into_response();
    // Pass through the upstream's Retry-After when we know the failure is a
    // rate-limit (429). Other libraries don't deserve the hint.
    if status == StatusCode::TOO_MANY_REQUESTS {
        if let Some(ra) = retry_after {
            if let Ok(v) = axum::http::HeaderValue::from_str(ra) {
                resp.headers_mut()
                    .insert(axum::http::header::RETRY_AFTER, v);
            }
        }
    }
    resp
}

/// Status codes that signal "this upstream is having a bad time, try the next
/// channel". 4xx other than these are the client's fault — no point failing
/// over to a second provider only to get the same 400.
fn is_retriable_status(code: u16) -> bool {
    code == 408 || code == 429 || code >= 500
}

/// Build the upstream request with auth + protocol-appropriate headers. The
/// caller still owns the body, so this is just a header recipe.
/// `upstream_protocol` is the protocol the upstream actually speaks (which
/// may differ from the client's when converting).
fn build_request(
    state: &AppState,
    base_url: &str,
    api_key: &str,
    upstream_protocol: &str,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    let req = if upstream_protocol == "anthropic" {
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

/// Pass the upstream body through unchanged, preserving the
/// drop-cancels-upstream mechanism: when the client disconnects, axum drops
/// the body, which drops the stream, which drops `resp`, which cancels the
/// upstream connection.
fn passthrough_stream(resp: reqwest::Response, status: StatusCode, content_type: Option<axum::http::HeaderValue>) -> Response {
    let body_stream = stream::unfold(resp, |mut r| async move {
        match r.chunk().await {
            Ok(Some(bytes)) => Some((Ok::<Bytes, reqwest::Error>(bytes), r)),
            Ok(None) => None,
            Err(e) => Some((Err(e), r)),
        }
    });
    let mut response = Response::builder().status(status);
    if let Some(ct) = content_type {
        if let Some(h) = response.headers_mut() {
            h.insert(axum::http::header::CONTENT_TYPE, ct);
        }
    }
    response
        .body(Body::from_stream(body_stream))
        .unwrap_or_else(|_| error_response(StatusCode::BAD_GATEWAY, "body build failed", None))
}

/// Convert an upstream SSE byte stream to the client's protocol, line by
/// line, via an `SseConverter`. Same drop-cancels-upstream property as the
/// passthrough stream.
fn converted_stream(resp: reqwest::Response, conv: Box<dyn SseConverter>) -> Response {
    let mut response = Response::builder().status(StatusCode::OK);
    if let Some(h) = response.headers_mut() {
        h.insert(axum::http::header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
    }
    let state = (resp, String::new(), conv, false);
    let body_stream = stream::unfold(state, |mut st| async move {
        let (resp, buf, conv, done) = (&mut st.0, &mut st.1, &mut st.2, &mut st.3);
        loop {
            if *done {
                return None;
            }
            if let Some(pos) = buf.find('\n') {
                let line = buf[..pos].trim_end_matches('\r').to_string();
                buf.drain(..=pos);
                if let Some(payload) = line.strip_prefix("data:") {
                    let payload = payload.trim();
                    let events = if payload == "[DONE]" {
                        *done = true;
                        conv.finish()
                    } else {
                        conv.on_data(payload)
                    };
                    if !events.is_empty() {
                        return Some((Ok::<Bytes, reqwest::Error>(Bytes::from(events.concat())), st));
                    }
                    if *done {
                        return None; // finish() produced nothing
                    }
                }
                // non-data lines (event:, comments, blanks) are dropped
                continue;
            }
            match resp.chunk().await {
                Ok(Some(bytes)) => {
                    buf.push_str(&String::from_utf8_lossy(&bytes));
                }
                Ok(None) => {
                    // upstream ended; flush converter's remaining events once
                    *done = true;
                    let events = conv.finish();
                    if events.is_empty() {
                        return None;
                    }
                    return Some((Ok(Bytes::from(events.concat())), st));
                }
                Err(e) => {
                    *done = true;
                    return Some((Err(e), st));
                }
            }
        }
    });
    response
        .body(Body::from_stream(body_stream))
        .unwrap_or_else(|_| error_response(StatusCode::BAD_GATEWAY, "body build failed", None))
}

/// Convert a successful upstream response into the client response, logging
/// the outcome against `channel_name`. Streaming requests pass through
/// unchanged; non-streaming responses are buffered so we can extract usage.
/// `convert` says which protocol translation this hop needs.
async fn respond_from_upstream(
    state: &AppState,
    token_name: &str,
    request_model: &str,
    model: &str,
    cand: &Candidate,
    resp: reqwest::Response,
    is_streaming: bool,
) -> Response {
    let channel_name = cand.name.as_str();
    let status =
        StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let convert = cand.convert;

    if !is_streaming {
        let content_type = resp.headers().get("content-type").cloned();
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => {
                log_request(
                    state,
                    token_name,
                    request_model,
                    model,
                    channel_name,
                    status.as_u16() as i64,
                    None,
                )
                .await;
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    &format!("upstream body read failed: {}", e),
                    None,
                );
            }
        };
        let usage = parse_usage(&bytes);
        log_request(
            state,
            token_name,
            request_model,
            model,
            channel_name,
            status.as_u16() as i64,
            usage,
        )
        .await;
        // translate the buffered body when converting
        let out_bytes = if convert != ConvertMode::None {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(v) => {
                    let converted = if status.is_success() {
                        match convert {
                            ConvertMode::ToOpenAI => convert::openai_resp_to_anthropic(&v, model),
                            ConvertMode::ToAnthropic => convert::anthropic_resp_to_openai(&v, model),
                            ConvertMode::None => unreachable!(),
                        }
                    } else {
                        match convert {
                            ConvertMode::ToOpenAI => convert::openai_err_to_anthropic(&v),
                            ConvertMode::ToAnthropic => convert::anthropic_err_to_openai(&v),
                            ConvertMode::None => unreachable!(),
                        }
                    };
                    serde_json::to_vec(&converted).unwrap_or_else(|_| bytes.to_vec())
                }
                Err(_) => bytes.to_vec(), // not JSON — pass through untouched
            }
        } else {
            bytes.to_vec()
        };
        let mut builder = Response::builder().status(status);
        if let Some(ct) = content_type {
            if let Some(h) = builder.headers_mut() {
                h.insert(axum::http::header::CONTENT_TYPE, ct);
            }
        }
        return builder
            .body(Body::from(out_bytes))
            .unwrap_or_else(|_| error_response(StatusCode::BAD_GATEWAY, "body build failed", None));
    }

    // Streaming: pass upstream through, log status only.
    log_request(
        state,
        token_name,
        request_model,
        model,
        channel_name,
        status.as_u16() as i64,
        None,
    )
    .await;
    match convert {
        ConvertMode::None => {
            let content_type = resp.headers().get("content-type").cloned();
            passthrough_stream(resp, status, content_type)
        }
        ConvertMode::ToOpenAI => {
            let conv = convert::OpenAiToAnthropicStream::new(model);
            converted_stream(resp, Box::new(conv))
        }
        ConvertMode::ToAnthropic => {
            let conv = convert::AnthropicToOpenAiStream::new(model);
            converted_stream(resp, Box::new(conv))
        }
    }
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
        None => return error_response(StatusCode::UNAUTHORIZED, "missing bearer token", None),
    };
    let token_name = match auth_token(state, &key).await {
        Ok(n) => n,
        Err((s, msg)) => return error_response(s, msg, None),
    };

    // 2. parse body to find model + streaming flag
    let req_json: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid JSON body", None),
    };
    let model = match req_json.get("model").and_then(|m| m.as_str()) {
        Some(m) => m.to_string(),
        None => return error_response(StatusCode::BAD_REQUEST, "missing model in request", None),
    };
    let targets = resolve_targets(state, &model).await;
    let is_streaming = req_json
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // 3+4. walk the ordered target list; for each (channel, model) target,
    // rewrite the request body's model field and try it — pinned targets go
    // to their channel only, unpinned ones fail over across every channel
    // serving that model. Fall through to the next target on failure.
    //
    // We track every candidate's failure (not just the last one) so the
    // final response can (a) report all of them in the body and (b) pick the
    // most informative status code — 429 if every failure was a rate-limit,
    // 504 if they were all transport errors, 502 otherwise. The first
    // upstream's `Retry-After` is captured so we can pass it through on 429.
    let mut all_errors: Vec<String> = Vec::new();
    let mut retry_after: Option<String> = None;
    let mut attempted = 0usize;
    let mut retriable_429_count = 0usize;
    let mut transport_err_count = 0usize;
    for (pin_channel, target_model) in &targets {
        let candidates = if pin_channel.is_empty() {
            match candidate_channels(state, target_model, protocol).await {
                Ok(c) => c,
                Err(s) => return error_response(s, "internal error", None),
            }
        } else {
            match pinned_channel(state, pin_channel, protocol).await {
                Some(c) => vec![c],
                None => {
                    all_errors.push(format!(
                        "pinned channel `{}` is disabled, missing, or serves neither protocol",
                        pin_channel
                    ));
                    continue;
                }
            }
        };
        if candidates.is_empty() {
            all_errors.push(format!("no enabled channel provides model `{}`", target_model));
            continue;
        }
        for cand in &candidates {
            // rewrite the request to the target model, in the upstream's
            // protocol when this channel needs conversion
            let upstream_protocol = match cand.convert {
                ConvertMode::ToOpenAI => "openai",
                ConvertMode::ToAnthropic => "anthropic",
                ConvertMode::None => protocol,
            };
            let body_json = match cand.convert {
                ConvertMode::None => {
                    let mut b = req_json.clone();
                    b["model"] = json!(target_model);
                    b
                }
                ConvertMode::ToOpenAI => {
                    convert::anthropic_req_to_openai(&req_json, target_model)
                }
                ConvertMode::ToAnthropic => {
                    convert::openai_req_to_anthropic(&req_json, target_model)
                }
            };
            let target_body = serde_json::to_vec(&body_json).unwrap_or_else(|_| body.to_vec());
            attempted += 1;
            let req = build_request(state, &cand.base_url, &cand.api_key, upstream_protocol, headers)
                .body(target_body);
            match req.send().await {
                Ok(resp) => {
                    let code = resp.status().as_u16();
                    if resp.status().is_success() {
                        return respond_from_upstream(
                            state,
                            &token_name,
                            &model,
                            target_model,
                            cand,
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
                            target_model,
                            cand,
                            resp,
                            is_streaming,
                        )
                        .await;
                    }
                    // retriable: capture Retry-After (first one wins), log, try next
                    if retry_after.is_none() {
                        if let Some(v) = resp.headers().get(reqwest::header::RETRY_AFTER) {
                            if let Ok(s) = v.to_str() {
                                retry_after = Some(s.to_string());
                            }
                        }
                    }
                    log_request(state, &token_name, &model, target_model, &cand.name, code as i64, None).await;
                    all_errors.push(format!("{} ({}) -> HTTP {}", cand.name, target_model, code));
                    if code == 429 {
                        retriable_429_count += 1;
                    }
                }
                Err(e) => {
                    log_request(state, &token_name, &model, target_model, &cand.name, -1, None).await;
                    all_errors.push(format!("{} ({}): {}", cand.name, target_model, e));
                    transport_err_count += 1;
                }
            }
        }
    }

    if attempted == 0 {
        // nothing even had a channel to try
        let wanted: Vec<String> = targets
            .iter()
            .map(|(c, m)| {
                if c.is_empty() {
                    m.clone()
                } else if m == "*" {
                    format!("{}/所有模型", c)
                } else {
                    format!("{}/{}", c, m)
                }
            })
            .collect();
        return error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "no enabled channel provides `{}` on the {} protocol",
                wanted.join("`, `"),
                protocol
            ),
            None,
        );
    }
    // Pick the most informative status code:
    //   - all retriable failures were 429 -> 429 (so SDKs that honor 429's
    //     Retry-After can back off correctly instead of blind exponential)
    //   - all failures were transport-level -> 504 (none of the upstreams
    //     even responded)
    //   - anything else (5xx mix, 429+5xx mix) -> 502 (gateway saw responses
    //     but couldn't serve the request)
    let final_status = if retriable_429_count == attempted {
        StatusCode::TOO_MANY_REQUESTS
    } else if transport_err_count == attempted {
        StatusCode::GATEWAY_TIMEOUT
    } else {
        StatusCode::BAD_GATEWAY
    };
    error_response(
        final_status,
        &format!(
            "all {} candidate(s) failed:\n  - {}",
            attempted,
            all_errors.join("\n  - ")
        ),
        retry_after.as_deref(),
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
        None => return error_response(StatusCode::UNAUTHORIZED, "missing bearer token", None),
    };
    if let Err((s, msg)) = auth_token(&state, &key).await {
        return error_response(s, msg, None);
    }
    // only external channels serve relay traffic (see candidate_channels)
    let rows = match sqlx::query(
        "SELECT models FROM channels WHERE enabled=1 AND kind='external'",
    )
        .fetch_all(&state.pool)
        .await
    {
        Ok(r) => r,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "db error", None),
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
        .map(|m| json!({ "id": m, "object": "model", "owned_by": "literouter" }))
        .collect();
    Json(json!({ "object": "list", "data": data })).into_response()
}