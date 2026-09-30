//! OpenAI-compatible relay: auth with internal token, route model -> channel,
//! forward request (streaming included) to the upstream channel. All incoming
//! requests are internal and are routed to enabled channels only (channels
//! with `enabled=0` are excluded from routing).
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
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
                "SELECT models FROM channels WHERE name=? AND enabled=1",
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
        "SELECT name, base_url, base_url_anthropic, api_key FROM channels WHERE name=? AND enabled=1",
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

/// Directory where debug request/response bodies are stored.
/// `data/debug_logs/{log_id}/` — created on first write for each log_id.
const DEBUG_LOG_DIR: &str = "data/debug_logs";

/// Write the upstream request body and response body to disk when debug
/// logging is enabled. Errors are silently ignored — debug logging is a
/// diagnostic aid, never a reason to fail a request.
/// Files: `data/debug_logs/{log_id}/req.json` and `resp.json`.
async fn write_debug_log(log_id: i64, req_body: &[u8], resp_body: &[u8]) {
    let dir = format!("{DEBUG_LOG_DIR}/{log_id}");
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        eprintln!("debug_log: create_dir {} failed: {}", dir, e);
        return;
    }
    async fn write(dir: &str, filename: &str, body: &[u8]) {
        let path = format!("{dir}/{filename}");
        if let Err(e) = tokio::fs::write(&path, body).await {
            eprintln!("debug_log: write {} failed: {}", path, e);
        }
    }
    write(&dir, "req.json", req_body).await;
    write(&dir, "resp.json", resp_body).await;
}

/// Delete the debug log directory for a given log_id. Idempotent —
/// directory may not exist. Called during cleanup so orphaned debug files
/// are removed when the corresponding log row is purged.
#[allow(dead_code)]
pub async fn delete_debug_log(log_id: i64) {
    let dir = format!("{DEBUG_LOG_DIR}/{log_id}");
    if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
        // ENOENT is fine — already gone
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("debug_log: remove_dir {} failed: {}", dir, e);
        }
    }
}

/// Everything we know about one relayed request, collected as the request
/// travels through the pipeline and written to `logs` once it settles.
/// Deliberately excludes the prompt and the reply text — this is metadata
/// only (routing, timing, token breakdown), so nothing user-authored lands
/// in the DB.
/// One upstream attempt inside a single client request. The client sees one
/// request and one log row; every channel we tried along the way is one of
/// these, including the one that finally won.
#[derive(Clone)]
struct Attempt {
    /// The model actually sent upstream, after mappings rewriting.
    upstream_model: String,
    channel_name: String,
    status: i64,
    error: String,
    /// Elapsed from the start of the request to this attempt's outcome. For
    /// failures this is cumulative (it includes the hops before it), matching
    /// what the old per-hop rows reported.
    latency_ms: i64,
    convert: ConvertMode,
    /// True when this attempt's response was the one returned to the client.
    ok: bool,
    /// Token breakdown. Only ever set on the winning attempt — a failed hop
    /// produced no usable response to read usage from, and a non-retriable 4xx
    /// has none either.
    usage: Option<convert::Usage>,
}

impl Attempt {
    fn new(
        upstream_model: &str,
        cand: &Candidate,
        status: i64,
        error: &str,
        latency_ms: i64,
        ok: bool,
    ) -> Self {
        Self {
            upstream_model: upstream_model.to_string(),
            channel_name: cand.name.clone(),
            status,
            error: error.to_string(),
            latency_ms,
            convert: cand.convert,
            ok,
            usage: None,
        }
    }

    fn with_usage(mut self, usage: Option<convert::Usage>) -> Self {
        self.usage = usage;
        self
    }

    fn with_error(mut self, error: String) -> Self {
        self.error = error;
        // The attempt failed — it was surfaced as a 502 to the client, so it
        // is not the "winning" hop, even though it was the last one tried.
        self.ok = false;
        self
    }

    fn usage_or_zero(&self) -> convert::Usage {
        self.usage.unwrap_or_default()
    }
}

/// The result of a client request, in the shape the `logs` row needs. Written
/// exactly once per request — by the streaming path from `Drop`, by the
/// buffered path inline.
struct LogEntry {
    token_name: String,
    /// The model the client asked for.
    request_model: String,
    protocol: String,
    streaming: bool,
    /// The attempt whose response reached the client. The parent row's model /
    /// channel / status / tokens all come from here, so the list page shows
    /// what the user actually got rather than the first thing we tried.
    winner: Option<Attempt>,
    /// Every attempt, in order, `winner` included. Parent + children go in
    /// one transaction so the list can never show a row whose children are
    /// missing.
    attempts: Vec<Attempt>,
}

/// Write the `logs` row plus one `log_attempts` row per hop, atomically.
/// Returns the `logs.id` on success so the caller can name debug files.
/// A no-op when there is no winner AND no attempts (nothing was tried).
async fn log_request(pool: &sqlx::SqlitePool, e: &LogEntry) -> Option<i64> {
    let Some(winner) = e.winner.as_ref().or_else(|| e.attempts.last()) else {
        return None;
    };
    let u = winner.usage_or_zero();
    let failed_count = e.attempts.iter().filter(|a| !a.ok).count() as i64;

    let mut tx = match pool.begin().await {
        Ok(t) => t,
        Err(_) => return None,
    };
    let inserted = match sqlx::query(
        "INSERT INTO logs (token_name, model, channel_name, status_code, prompt_tokens, completion_tokens, total_tokens, created_at, request_model, latency_ms, stream, protocol, convert, upstream_model, error, cache_read_tokens, cache_creation_tokens, reasoning_tokens, failed_count) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&e.token_name)
    .bind(&winner.upstream_model)
    .bind(&winner.channel_name)
    .bind(winner.status)
    .bind(u.prompt)
    .bind(u.completion)
    .bind(u.total)
    .bind(now())
    .bind(&e.request_model)
    .bind(winner.latency_ms)
    .bind(e.streaming as i64)
    .bind(&e.protocol)
    .bind(convert_label(winner.convert))
    .bind(&winner.upstream_model)
    .bind(&winner.error)
    .bind(u.cache_read)
    .bind(u.cache_creation)
    .bind(u.reasoning)
    .bind(failed_count)
    .execute(&mut *tx)
    .await
    {
        Ok(r) => r,
        Err(_) => return None,
    };
    let log_id = inserted.last_insert_rowid();

    for (seq, a) in e.attempts.iter().enumerate() {
        let _ = sqlx::query(
            "INSERT INTO log_attempts (log_id, seq, upstream_model, channel_name, status_code, error, latency_ms, convert, ok) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(log_id)
        .bind(seq as i64)
        .bind(&a.upstream_model)
        .bind(&a.channel_name)
        .bind(a.status)
        .bind(&a.error)
        .bind(a.latency_ms)
        .bind(convert_label(a.convert))
        .bind(a.ok as i64)
        .execute(&mut *tx)
        .await;
    }
    if tx.commit().await.is_err() {
        return None;
    }
    Some(log_id)
}

fn convert_label(mode: ConvertMode) -> &'static str {
    match mode {
        ConvertMode::None => "none",
        ConvertMode::ToOpenAI => "to_openai",
        ConvertMode::ToAnthropic => "to_anthropic",
    }
}

/// Extract the token breakdown from an upstream JSON body (non-streaming).
fn parse_usage(body: &[u8]) -> Option<convert::Usage> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let u = convert::parse_usage_obj(v.get("usage")?);
    if u.is_empty() {
        return None;
    }
    Some(u)
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
/// Owns everything needed to write the final log row once the stream ends.
/// The stream closure captures one of these by value; when the upstream body
/// exhausts, the closure spawns a tokio task that calls `log_request` with
/// the captured usage. This lets the response body be streamed straight to
/// the client (no buffering) while still recording the eventual token count.
///
/// `LogEntry` is already fully owned, so it moves into the spawned task as-is.
struct StreamLog {
    pool: sqlx::SqlitePool,
    entry: LogEntry,
    /// The upstream request body, written to disk when debug logging is on.
    req_body: Bytes,
}

impl StreamLog {
    /// Finalize the log row and (if `state.debug_logging`) write the request
    /// and response bodies to disk. Usage and the accumulated response bytes are
    /// supplied by the caller (Drop impl of LogOnEnd).
    async fn spawn_inline(
        mut self,
        usage: Option<convert::Usage>,
        resp_buf: Arc<Mutex<Vec<u8>>>,
    ) {
        if let Some(winner) = self.entry.winner.as_mut() {
            winner.usage = usage;
        }
        let log_id = log_request(&self.pool, &self.entry).await;
        if state_debug_logging() && log_id.is_some() {
            let req = self.req_body.to_vec();
            let resp = resp_buf.lock().unwrap().clone();
            write_debug_log(log_id.unwrap(), &req, &resp).await;
        }
    }
}

/// Whether debug logging is globally enabled. Stored in a static so we avoid a
/// DB lookup on every request; updated atomically when an admin toggles it.
static DEBUG_LOGGING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set the global debug_logging flag. Called at startup and whenever an admin
/// toggles the switch.
pub fn set_debug_logging(v: bool) {
    DEBUG_LOGGING.store(v, std::sync::atomic::Ordering::Relaxed);
}

fn state_debug_logging() -> bool {
    DEBUG_LOGGING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Wraps a byte stream and spawns a log task when the wrapper is dropped.
/// Drop fires on three paths:
///   1. upstream ended cleanly → Poll::Ready(None)
///   2. axum's response body finished streaming to the client (success)
///   3. the client disconnected mid-stream → upstream gets cancelled, the
///      body_stream future is dropped
/// All three are when we want to record the row, so Drop is the right hook.
///
/// Usage source at Drop time depends on which constructor was used:
///   - `wrap()` — the passthrough unfold has been writing usage into the
///     shared `usage` mutex as it scanned `data:` lines
///   - `wrap_with_converter()` — the SseConverter tracks usage as a side
///     effect of translating, so we read `converter.usage()` directly
struct LogOnEnd<S> {
    inner: S,
    log: Option<StreamLog>,
    usage: Arc<Mutex<Option<convert::Usage>>>,
    /// If set, overrides `usage` when Drop fires. Used for the converted
    /// path so the converter's own usage counter is the source of truth.
    converter_usage: Option<Arc<Mutex<Box<dyn SseConverter>>>>,
    /// Full response body accumulated during streaming. Read at Drop and
    /// forwarded to `StreamLog` for the debug log file.
    resp_buf: Arc<Mutex<Vec<u8>>>,
}

impl<S> LogOnEnd<S> {
    fn wrap(
        inner: S,
        log: StreamLog,
        usage: Arc<Mutex<Option<convert::Usage>>>,
        resp_buf: Arc<Mutex<Vec<u8>>>,
    ) -> Self {
        Self { inner, log: Some(log), usage, converter_usage: None, resp_buf }
    }
    fn wrap_with_converter(
        inner: S,
        log: StreamLog,
        converter: Arc<Mutex<Box<dyn SseConverter>>>,
        resp_buf: Arc<Mutex<Vec<u8>>>,
    ) -> Self {
        Self {
            inner,
            log: Some(log),
            usage: Arc::new(Mutex::new(None)),
            converter_usage: Some(converter),
            resp_buf,
        }
    }
}

impl<S> futures_util::Stream for LogOnEnd<S>
where
    S: futures_util::Stream<Item = Result<Bytes, reqwest::Error>>,
{
    type Item = S::Item;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        // SAFETY: `LogOnEnd` doesn't move `inner` out of `self`. The
        // projection through `get_unchecked_mut` is sound because
        // `inner` is structurally pinned (we never hand out &mut Self
        // to anyone else). We re-pin with `new_unchecked` because
        // the upstream byte stream (e.g. `stream::unfold` over
        // `reqwest::Response`) may not be `Unpin`.
        let this = unsafe { self.get_unchecked_mut() };
        let inner = unsafe { std::pin::Pin::new_unchecked(&mut this.inner) };
        inner.poll_next(cx)
    }
}

impl<S> Drop for LogOnEnd<S> {
    fn drop(&mut self) {
        if let Some(log) = self.log.take() {
            let usage = if let Some(conv) = self.converter_usage.as_ref() {
                conv.lock().unwrap().usage()
            } else {
                self.usage.lock().unwrap().clone()
            };
            let resp_buf = self.resp_buf.clone();
            tokio::spawn(async move {
                log.spawn_inline(usage, resp_buf).await;
            });
        }
    }
}

/// Pull the token breakdown out of one SSE `data:` payload. Implemented
/// in `convert.rs` since that's where the OpenAI/Anthropic field-name
/// knowledge already lives; re-exported here for the passthrough path.
fn usage_from_sse_payload(payload: &str) -> Option<convert::Usage> {
    convert::usage_from_sse_payload(payload)
}

/// Forward an upstream SSE byte stream to the client verbatim, but parse
/// `data:` lines on the side so we can record token usage when the stream
/// ends. Same drop-cancels-upstream property as the converted stream.
fn passthrough_stream(
    resp: reqwest::Response,
    status: StatusCode,
    content_type: Option<axum::http::HeaderValue>,
    log: StreamLog,
    resp_buf: Arc<Mutex<Vec<u8>>>,
) -> Response {
    let usage: Arc<Mutex<Option<convert::Usage>>> = Arc::new(Mutex::new(None));
    let usage_for_drop = Arc::clone(&usage);
    let body_stream = stream::unfold(
        (resp, Vec::<u8>::new(), Arc::clone(&usage)),
        |mut st| async move {
            let (resp, buf, usage_ref) = (&mut st.0, &mut st.1, &mut st.2);
            match resp.chunk().await {
                Ok(Some(bytes)) => {
                    // Copy the bytes into buf for side-effect usage
                    // extraction, then forward the original chunk to the
                    // client untouched. Copying (not moving) lets us
                    // hand `bytes` straight to axum while keeping the
                    // parse buffer authoritative for line scanning.
                    buf.extend_from_slice(&bytes);
                    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=pos).collect();
                        let line_str = std::str::from_utf8(&line).unwrap_or("");
                        let trimmed = line_str.trim_end_matches('\r');
                        if let Some(payload) = trimmed.strip_prefix("data:") {
                            if let Some(u) = usage_from_sse_payload(payload.trim()) {
                                *usage_ref.lock().unwrap() = Some(u);
                            }
                        }
                    }
                    Some((Ok::<Bytes, reqwest::Error>(bytes), st))
                }
                Ok(None) => None,
                Err(e) => Some((Err(e), st)),
            }
        },
    );
    let mut response = Response::builder().status(status);
    if let Some(ct) = content_type {
        if let Some(h) = response.headers_mut() {
            h.insert(axum::http::header::CONTENT_TYPE, ct);
        }
    }
    response
        .body(Body::from_stream(LogOnEnd::wrap(
            body_stream,
            log,
            usage_for_drop,
            resp_buf,
        )))
        .unwrap_or_else(|_| error_response(StatusCode::BAD_GATEWAY, "body build failed", None))
}

/// Convert an upstream SSE byte stream to the client's protocol, line by
/// line, via an `SseConverter`. Same drop-cancels-upstream property as the
/// passthrough stream. The converter is shared via `Arc<Mutex<>>` so the
/// final token usage can be read out from `converter.usage()` when the
/// stream ends — the converter already tracks usage as a side-effect of
/// translating each chunk, so we don't need a second SSE parser here.
fn converted_stream(
    resp: reqwest::Response,
    conv: Box<dyn SseConverter>,
    log: StreamLog,
) -> Response {
    let conv: Arc<Mutex<Box<dyn SseConverter>>> = Arc::new(Mutex::new(conv));
    let mut response = Response::builder().status(StatusCode::OK);
    if let Some(h) = response.headers_mut() {
        h.insert(axum::http::header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
    }
    let state = (resp, String::new(), Arc::clone(&conv), false);
    let body_stream = stream::unfold(state, |mut st| async move {
        let (resp, buf, conv_ref, done) =
            (&mut st.0, &mut st.1, &mut st.2, &mut st.3);
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
                        conv_ref.lock().unwrap().finish()
                    } else {
                        conv_ref.lock().unwrap().on_data(payload)
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
                    let events = conv_ref.lock().unwrap().finish();
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
        .body(Body::from_stream(LogOnEnd::wrap_with_converter(
            body_stream,
            log,
            conv,
            Arc::new(Mutex::new(Vec::new())),
        )))
        .unwrap_or_else(|_| error_response(StatusCode::BAD_GATEWAY, "body build failed", None))
}

/// Convert an upstream response into the client response, returning the
/// `Attempt` that produced it so the caller can record it as the winning hop
/// of this request. This function does not log — logging is the caller's job,
/// because only the caller knows about the hops that failed before this one.
///
/// Streaming requests pass through unchanged; non-streaming responses are
/// buffered so we can extract usage. `convert` says which protocol
/// translation this hop needs. `start` is when the relay started, so we can
/// record how long the upstream took to first respond (TTFB); on stream replay
/// it's frozen at the transition.
#[allow(clippy::too_many_arguments)]
async fn respond_from_upstream(
    state: &AppState,
    token_name: &str,
    request_model: &str,
    model: &str,
    cand: &Candidate,
    resp: reqwest::Response,
    is_streaming: bool,
    protocol: &str,
    streaming: bool,
    start: Instant,
    attempts: Vec<Attempt>,
    // The body that was actually sent upstream (post-mappings rewrite).
    req_body: Bytes,
) -> (Response, Attempt) {
    let status =
        StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    // Latency up to "we got the upstream's response headers". For streams
    // this is the TTFB, which is what people usually want; for buffered
    // responses this is the whole round-trip.
    let latency_ms = start.elapsed().as_millis() as i64;
    // This hop ends the request either way — the client gets its response
    // here, whether it succeeded or was a non-retriable error.
    let attempt = Attempt::new(model, cand, status.as_u16() as i64, "", latency_ms, true);

    if !is_streaming {
        let content_type = resp.headers().get("content-type").cloned();
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => {
                let attempt = attempt.with_error(format!("upstream body read failed: {}", e));
                let entry = LogEntry {
                    token_name: token_name.to_string(),
                    request_model: request_model.to_string(),
                    protocol: protocol.to_string(),
                    streaming,
                    winner: Some(attempt.clone()),
                    attempts: push(attempts, attempt.clone()),
                };
                let _ = log_request(&state.pool, &entry).await;
                return (
                    error_response(
                        StatusCode::BAD_GATEWAY,
                        &format!("upstream body read failed: {}", e),
                        None,
                    ),
                    attempt,
                );
            }
        };
        let attempt = attempt.with_usage(parse_usage(&bytes));
        // translate the buffered body when converting
        let out_bytes = if cand.convert != ConvertMode::None {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(v) => {
                    let converted = if status.is_success() {
                        match cand.convert {
                            ConvertMode::ToOpenAI => {
                                convert::openai_resp_to_anthropic(&v, model)
                            }
                            ConvertMode::ToAnthropic => {
                                convert::anthropic_resp_to_openai(&v, model)
                            }
                            ConvertMode::None => unreachable!(),
                        }
                    } else {
                        match cand.convert {
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
        let entry = LogEntry {
            token_name: token_name.to_string(),
            request_model: request_model.to_string(),
            protocol: protocol.to_string(),
            streaming,
            winner: Some(attempt.clone()),
            attempts: push(attempts, attempt.clone()),
        };
        let log_id = log_request(&state.pool, &entry).await;
        if state.debug_logging {
            let _ = write_debug_log(log_id.unwrap_or(0), &req_body, &out_bytes).await;
        }
        let mut builder = Response::builder().status(status);
        if let Some(ct) = content_type {
            if let Some(h) = builder.headers_mut() {
                h.insert(axum::http::header::CONTENT_TYPE, ct);
            }
        }
        let resp = builder
            .body(Body::from(out_bytes))
            .unwrap_or_else(|_| error_response(StatusCode::BAD_GATEWAY, "body build failed", None));
        return (resp, attempt);
    }

    // Streaming: forward to the client. The stream wrapper owns the log row
    // and writes it when the body stream is dropped (i.e., when the upstream
    // ends, the client finishes, or the client disconnects). It carries the
    // preceding failed hops so the detail page can show the whole chain.
    let resp_buf = Arc::new(Mutex::new(Vec::new()));
    let log = StreamLog {
        pool: state.pool.clone(),
        entry: LogEntry {
            token_name: token_name.to_string(),
            request_model: request_model.to_string(),
            protocol: protocol.to_string(),
            streaming,
            winner: Some(attempt.clone()),
            attempts: push(attempts, attempt.clone()),
        },
        req_body,
    };
    let resp = match cand.convert {
        ConvertMode::None => {
            let content_type = resp.headers().get("content-type").cloned();
            passthrough_stream(resp, status, content_type, log, resp_buf)
        }
        ConvertMode::ToOpenAI => {
            let conv = convert::OpenAiToAnthropicStream::new(model);
            converted_stream(resp, Box::new(conv), log)
        }
        ConvertMode::ToAnthropic => {
            let conv = convert::AnthropicToOpenAiStream::new(model);
            converted_stream(resp, Box::new(conv), log)
        }
    };
    (resp, attempt)
}

/// Append one hop to the chain, returning the new vector. Cheaper to read at
/// the call sites than `let mut v = attempts; v.push(a); v`.
fn push(mut attempts: Vec<Attempt>, a: Attempt) -> Vec<Attempt> {
    attempts.push(a);
    attempts
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
    // One clock per request, started after auth+parse so the logged
    // latency reflects only the upstream work.
    let relay_start = Instant::now();

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
    // Every upstream attempt so far, in order. Recorded as child rows of the
    // single `logs` row this request produces. A hop that ends the request
    // (success or non-retriable error) is pushed by `respond_from_upstream`.
    let mut attempts: Vec<Attempt> = Vec::new();
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
                .body(target_body.clone());
            match req.send().await {
                Ok(resp) => {
                    let code = resp.status().as_u16();
                    let req_body = Bytes::from(target_body.clone());
                    if resp.status().is_success() {
                        return respond_from_upstream(
                            state,
                            &token_name,
                            &model,
                            target_model,
                            cand,
                            resp,
                            is_streaming,
                            protocol,
                            is_streaming,
                            relay_start,
                            attempts,
                            req_body,
                        )
                        .await
                        .0;
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
                            protocol,
                            is_streaming,
                            relay_start,
                            attempts,
                            req_body,
                        )
                        .await
                        .0;
                    }
                    // retriable: capture Retry-After (first one wins), record
                    // the hop, try the next candidate
                    if retry_after.is_none() {
                        if let Some(v) = resp.headers().get(reqwest::header::RETRY_AFTER) {
                            if let Ok(s) = v.to_str() {
                                retry_after = Some(s.to_string());
                            }
                        }
                    }
                    let err_msg = format!("HTTP {}", code);
                    attempts.push(Attempt::new(
                        target_model,
                        cand,
                        code as i64,
                        &err_msg,
                        relay_start.elapsed().as_millis() as i64,
                        false,
                    ));
                    all_errors.push(format!("{} ({}) -> {}", cand.name, target_model, err_msg));
                    if code == 429 {
                        retriable_429_count += 1;
                    }
                }
                Err(e) => {
                    let err_msg = format!("transport: {}", e);
                    attempts.push(Attempt::new(
                        target_model,
                        cand,
                        -1,
                        &err_msg,
                        relay_start.elapsed().as_millis() as i64,
                        false,
                    ));
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
        let err = format!(
            "no enabled channel provides `{}` on the {} protocol",
            wanted.join("`, `"),
            protocol
        );
        // Still a client request worth a log row: "this model isn't served
        // anywhere" is exactly the question the log page answers. No upstream
        // was contacted, so the single attempt carries the routing failure
        // rather than an HTTP response.
        let entry = LogEntry {
            token_name: token_name.clone(),
            request_model: model.clone(),
            protocol: protocol.to_string(),
            streaming: is_streaming,
            winner: Some(Attempt {
                upstream_model: targets
                    .first()
                    .map(|(_, m)| m.clone())
                    .unwrap_or_default(),
                channel_name: String::new(),
                status: StatusCode::NOT_FOUND.as_u16() as i64,
                error: err.clone(),
                latency_ms: relay_start.elapsed().as_millis() as i64,
                convert: ConvertMode::None,
                ok: false,
                usage: None,
            }),
            attempts: Vec::new(),
        };
        let _ = log_request(&state.pool, &entry).await;
        return error_response(StatusCode::NOT_FOUND, &err, None);
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
    let err = format!(
        "all {} candidate(s) failed:\n  - {}",
        attempted,
        all_errors.join("\n  - ")
    );
    // Every attempt failed. Create a synthetic "all-failed" winner with an
    // empty channel_name so the parent row shows "—" for the channel rather
    // than misleadingly naming the last failed channel. The actual failure
    // chain is preserved in `attempts` for the detail page.
    let all_failed = Attempt {
        upstream_model: attempts
            .last()
            .map(|a| a.upstream_model.clone())
            .unwrap_or_default(),
        channel_name: String::new(),
        status: final_status.as_u16() as i64,
        error: err.clone(),
        latency_ms: relay_start.elapsed().as_millis() as i64,
        convert: ConvertMode::None,
        ok: false,
        usage: None,
    };
    let entry = LogEntry {
        token_name,
        request_model: model,
        protocol: protocol.to_string(),
        streaming: is_streaming,
        winner: Some(all_failed),
        attempts,
    };
    log_request(&state.pool, &entry).await;
    error_response(
        final_status,
        &err,
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
        "SELECT models FROM channels WHERE enabled=1",
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