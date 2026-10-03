//! OpenAI-compatible relay: auth with internal token, route model -> channel,
//! forward request (streaming included) to the upstream channel. All incoming
//! requests are internal and are routed to enabled channels only (channels
//! with `enabled=0` are excluded from routing).
//!
//! Failover: every non-2xx response — transport errors, 5xx, **and 4xx
//! including 404 model-not-found** — is treated as a fallback signal and
//! the relay moves to the next candidate/target rather than returning the
//! error to the client. This is the point of `model_mappings`: an upstream
//! going away, a model being renamed, or a quota misconfiguration should
//! not bring down the whole request when other targets are available.
//!
//! Across requests, the per-(channel, model) circuit breaker remembers
//! which combinations are broken and short-circuits them, so a single
//! broken upstream doesn't burn quota on every subsequent request. The
//! breaker tracks 4xx (404/401/403) too — those represent stable
//! (channel, model) incompatibilities, not transient health issues.

use crate::admin;
use crate::breaker::{self, Outcome};
use crate::breaker_history::{record_breaker_event, BreakerEventKind, BreakerEventRow};
use crate::convert::{self, ConvertMode, SseConverter};
use crate::db::now;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use futures_util::stream;
use serde_json::{json, Value};
use sqlx::Row;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Peer address, or `None` when the server wasn't built with
/// `into_make_service_with_connect_info`.
///
/// `axum::extract::ConnectInfo` itself rejects the request outright when the
/// extension is absent, which makes the relay untestable in-process. This
/// mirrors its lookup but degrades to `None`: in production the extension is
/// always present, so behavior is unchanged, and a request from a
/// reverse-proxied setup without `X-Forwarded-For` degrades to "no client IP"
/// instead of a 500.
pub struct OptionalConnectInfo(pub Option<std::net::SocketAddr>);

#[axum::async_trait]
impl<S> FromRequestParts<S> for OptionalConnectInfo
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(OptionalConnectInfo(
            parts
                .extensions
                .get::<ConnectInfo<std::net::SocketAddr>>()
                .map(|ci| ci.0),
        ))
    }
}

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
    let row =
        sqlx::query("SELECT name, enabled, rpm_limit, daily_token_limit FROM tokens WHERE key=?")
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
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM logs WHERE token_name=? AND created_at >= ?")
                .bind(&name)
                .bind(since)
                .fetch_one(&state.pool)
                .await
                .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "db error"))?;
        if count >= rpm_limit {
            return Err((StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded (rpm)"));
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
            return Err((StatusCode::TOO_MANY_REQUESTS, "daily token quota exceeded"));
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
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT targets, target_model FROM model_mappings WHERE alias=?")
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
            if let Ok(rows) = sqlx::query("SELECT models FROM channels WHERE name=? AND enabled=1")
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
async fn pinned_channel(state: &AppState, name: &str, protocol: &str) -> Option<Candidate> {
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

/// Default directory where captured response bodies are stored:
/// `data/debug_logs/{log_id}/` — created on first write for each log_id.
const DEBUG_LOG_DIR: &str = "data/debug_logs";

/// Per-pool debug directory overrides. Indexed by the SQLite file path the
/// pool is using, which is unique per `TestDb` and therefore per test.
/// Without it, parallel tests would race: each pool starts at row id 1 and both
/// would write to the same `dir/1/resp.json`. The map is keyed by the
/// database path (rather than the pool pointer) so two pools sharing a
/// database — for instance the prod app and a migration probe — share a
/// capture directory, which is the right semantics.
static DEBUG_LOG_DIR_OVERRIDE: std::sync::LazyLock<
    Mutex<std::collections::HashMap<String, String>>,
> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Set the per-pool debug log directory. Test fixture only.
pub fn set_debug_log_dir_for(pool: &sqlx::SqlitePool, dir: &str) {
    let key = pool_key(pool);
    DEBUG_LOG_DIR_OVERRIDE
        .lock()
        .unwrap()
        .insert(key, dir.to_string());
}

fn debug_log_dir_for(pool: &sqlx::SqlitePool) -> String {
    let key = pool_key(pool);
    DEBUG_LOG_DIR_OVERRIDE
        .lock()
        .unwrap()
        .get(&key)
        .cloned()
        .unwrap_or_else(|| DEBUG_LOG_DIR.to_string())
}

/// Stable identifier for the SQLite file backing this pool. Two pools from
/// the same `TestDb` would otherwise share captures; tests that need
/// isolation get isolation via the SQLite path, which `TestDb` makes unique.
fn pool_key(pool: &sqlx::SqlitePool) -> String {
    // `connect_options` returns `Arc<SqliteConnectOptions>`; `get_filename`
    // takes `self` by value so we clone the inner options to read the path.
    pool.connect_options()
        .as_ref()
        .clone()
        .get_filename()
        .to_string_lossy()
        .into_owned()
}

/// Upper bound on a captured response body. A failing upstream can send
/// anything — an HTML error page, a multi-megabyte dump — and the log detail
/// page has no use for more than the first chunk of it. Past the cap we stop
/// accumulating and mark the capture truncated so the reader knows they're
/// looking at a fragment.
pub const DEBUG_BODY_MAX: usize = 256 * 1024;

#[derive(Default)]
struct CaptureInner {
    bytes: Vec<u8>,
    /// Total bytes offered to `push`, including the ones dropped past the
    /// cap — the log page shows "of N bytes" so a truncated capture is
    /// visibly incomplete rather than silently short.
    seen: usize,
    truncated: bool,
}

/// A handle on the upstream response body, filled as it streams past and read
/// once the request settles. Cloning shares the buffer, so the SSE pump and
/// the log-writing task can each hold one.
///
/// Deliberately narrow: it captures the **upstream** bytes and nothing else.
/// The request body is never offered to it — see `write_debug_log`.
#[derive(Clone, Default)]
pub struct DebugCapture(Arc<Mutex<CaptureInner>>);

impl DebugCapture {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a chunk. Cheap and non-blocking (a `std::sync::Mutex`, never
    /// held across an await) so it's safe to call from inside the unfold
    /// closure driving a live stream.
    pub fn push(&self, chunk: &[u8]) {
        let mut inner = self.0.lock().unwrap();
        inner.seen += chunk.len();
        let room = DEBUG_BODY_MAX.saturating_sub(inner.bytes.len());
        if room == 0 {
            inner.truncated = true;
            return;
        }
        let take = chunk.len().min(room);
        inner.bytes.extend_from_slice(&chunk[..take]);
        if take < chunk.len() {
            inner.truncated = true;
        }
    }

    /// `(captured bytes, total bytes seen, truncated)`.
    pub fn snapshot(&self) -> (Vec<u8>, usize, bool) {
        let inner = self.0.lock().unwrap();
        (inner.bytes.clone(), inner.seen, inner.truncated)
    }
}

/// One captured body plus the metadata `resp.meta.json` records alongside it.
pub struct DebugCaptureFile {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

/// Write the captured **response** body to disk.
///
/// The request body is deliberately not written. Migration
/// `0014_log_details.sql` records the stance — prompts and replies are the
/// user's private data, and diagnosing a relay fault needs "what did the
/// upstream actually send", not what we sent it. Not writing it is the
/// strongest form of that: there is no code path that could leak it.
///
/// Files: `data/debug_logs/{log_id}/resp.json` and `resp.meta.json`.
/// Errors are swallowed — a diagnostic aid never fails a request.
pub async fn write_debug_log(pool: &sqlx::SqlitePool, log_id: i64, capture: &DebugCapture) {
    let (bytes, seen, truncated) = capture.snapshot();
    if bytes.is_empty() {
        return;
    }
    let dir = format!("{}/{log_id}", debug_log_dir_for(pool));
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
    write(&dir, "resp.json", &bytes).await;
    let meta = json!({
        "bytes": seen,
        "captured": bytes.len(),
        "truncated": truncated,
        "max_bytes": DEBUG_BODY_MAX,
        "at": now(),
    });
    write(
        &dir,
        "resp.meta.json",
        serde_json::to_vec_pretty(&meta)
            .unwrap_or_default()
            .as_slice(),
    )
    .await;
}

/// Read back a previously captured body. `None` when nothing was captured
/// for this log id — which is the normal case, since we only write on failure
/// or when debug logging is on.
pub async fn read_debug_log(pool: &sqlx::SqlitePool, log_id: i64) -> Option<DebugCaptureFile> {
    let dir = format!("{}/{log_id}", debug_log_dir_for(pool));
    let bytes = tokio::fs::read(format!("{dir}/resp.json")).await.ok()?;
    let truncated = match tokio::fs::read(format!("{dir}/resp.meta.json")).await {
        Ok(meta) => serde_json::from_slice::<Value>(&meta)
            .ok()
            .and_then(|v| v.get("truncated").and_then(|b| b.as_bool()))
            .unwrap_or(false),
        Err(_) => false,
    };
    Some(DebugCaptureFile { bytes, truncated })
}

/// Whether a debug capture exists for this log id, without reading the body.
/// The detail page uses it to decide whether to render the section at all, so
/// a request that captured nothing costs one `stat` instead of a button and a
/// round trip that comes back empty.
pub async fn debug_log_exists(pool: &sqlx::SqlitePool, log_id: i64) -> bool {
    let dir = format!("{}/{log_id}", debug_log_dir_for(pool));
    tokio::fs::metadata(format!("{dir}/resp.json"))
        .await
        .is_ok()
}

/// Delete the debug log directory for a given log_id. Idempotent —
/// directory may not exist. Called by `db::cleanup_old_logs` so the files
/// under a purged `logs` row go with it.
pub async fn delete_debug_log(pool: &sqlx::SqlitePool, log_id: i64) {
    let dir = format!("{}/{log_id}", debug_log_dir_for(pool));
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
    /// True when this hop was skipped because the breaker was open — no HTTP
    /// call was made, the upstream never saw the request. Kept distinct from
    /// `ok = false` (a real upstream failure) so the log can separate
    /// "deliberately didn't try" from "tried and failed". Excluded from
    /// `failed_count` and from the failed-attempts badge in the UI.
    skipped: bool,
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
            skipped: false,
            usage: None,
        }
    }

    /// Mark this hop as a breaker-skipped routing decision — no HTTP call
    /// was made, so it shouldn't be counted as a "real" failure. Still
    /// surfaced in the attempts table so the admin can see the relay walked
    /// past this (channel, model) without trying.
    fn skipped(mut self) -> Self {
        self.skipped = true;
        self
    }

    fn with_usage(mut self, usage: Option<convert::Usage>) -> Self {
        self.usage = usage;
        self
    }

    fn usage_or_zero(&self) -> convert::Usage {
        self.usage.unwrap_or_default()
    }
}

/// Client IP and User-Agent, extracted once at request entry and threaded
/// through to every LogEntry so the log page can show the call source.
/// IP is resolved from X-Forwarded-For (reverse proxy) or the direct TCP
/// connection, preferring the former so proxy setups are respected.
#[derive(Clone)]
struct ClientInfo {
    ip: String,
    user_agent: String,
}

fn extract_client_info(headers: &HeaderMap, direct_ip: Option<std::net::SocketAddr>) -> ClientInfo {
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| direct_ip.map(|a| a.ip().to_string()))
        .unwrap_or_default();
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    ClientInfo { ip, user_agent }
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
    client_ip: String,
    user_agent: String,
}

/// Write the `logs` row plus one `log_attempts` row per hop, atomically.
/// Returns the `logs.id` on success so the caller can name debug files.
/// A no-op when there is no winner AND no attempts (nothing was tried).
async fn log_request(pool: &sqlx::SqlitePool, e: &LogEntry) -> Option<i64> {
    let winner = e.winner.as_ref().or_else(|| e.attempts.last())?;
    let u = winner.usage_or_zero();
    // Skipped hops are breaker decisions, not upstream failures — don't pollute
    // the visible failure count with "we deliberately didn't try this".
    let failed_count = e.attempts.iter().filter(|a| !a.ok && !a.skipped).count() as i64;

    let mut tx = match pool.begin().await {
        Ok(t) => t,
        Err(_) => return None,
    };
    let inserted = match sqlx::query(
        "INSERT INTO logs (token_name, model, channel_name, status_code, prompt_tokens, completion_tokens, total_tokens, created_at, request_model, latency_ms, stream, protocol, convert, upstream_model, error, cache_read_tokens, cache_creation_tokens, reasoning_tokens, failed_count, client_ip, user_agent) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    .bind(&e.client_ip)
    .bind(&e.user_agent)
    .execute(&mut *tx)
    .await
    {
        Ok(r) => r,
        Err(_) => return None,
    };
    let log_id = inserted.last_insert_rowid();

    for (seq, a) in e.attempts.iter().enumerate() {
        let _ = sqlx::query(
            "INSERT INTO log_attempts (log_id, seq, upstream_model, channel_name, status_code, error, latency_ms, convert, ok, skipped) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(a.skipped as i64)
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

/// The `error.type` an Anthropic client expects for a given status.
///
/// The Messages API spec fixes a closed set of these and SDKs key retry
/// behavior off them, so a gateway on `/v1/messages` has to speak them rather
/// than inventing its own string.
fn anthropic_error_type(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "invalid_request_error",
        StatusCode::UNAUTHORIZED => "authentication_error",
        StatusCode::FORBIDDEN => "permission_error",
        StatusCode::NOT_FOUND => "not_found_error",
        StatusCode::PAYLOAD_TOO_LARGE => "request_too_large",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        _ => "api_error",
    }
}

/// The error envelope for `protocol`.
///
/// The two protocols disagree on the shape, not just the wording: Anthropic
/// wraps the error in a top-level `"type": "error"` discriminator, while the
/// OpenAI-compatible shape is what `/v1/*` clients have always seen. Handing
/// an Anthropic caller the OpenAI shape still yields a readable message (SDKs
/// fall back to a generic `APIError`), but the `type` is then unusable for the
/// retry decisions it exists to drive.
fn error_response(
    protocol: &str,
    status: StatusCode,
    message: &str,
    retry_after: Option<&str>,
) -> Response {
    let body = if protocol == "anthropic" {
        json!({
            "type": "error",
            "error": { "type": anthropic_error_type(status), "message": message }
        })
    } else {
        json!({
            "error": { "message": message, "type": "literouter_error" }
        })
    };
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

/// Outcome of one upstream attempt, classified into four buckets so the
/// caller can dispatch without re-checking status codes. The relay never
/// returns a failure to the client — every `Http` / `InvalidBody` /
/// `Transport` outcome is a fallback signal.
enum UpstreamOutcome {
    /// 2xx with a body of the expected shape. `Buffered` for non-streaming
    /// (already read and validated), `Live` for streaming (handed straight to
    /// the SSE pump, body still unread).
    Ok(UpstreamBody),
    /// Non-2xx with a parsed HTTP status. `retry_after_secs` is the
    /// upstream's `Retry-After` header parsed as integer seconds (only
    /// the form defined by RFC 7231 §7.1.3 is supported; HTTP-date form
    /// falls back to `None` because LLM providers don't use it).
    Http {
        status: u16,
        retry_after_secs: Option<u64>,
        /// Best-effort human-readable cause pulled out of the upstream's
        /// error body, e.g. `"Rate limit exceeded"`. `None` when the body
        /// was empty or unreadable.
        detail: Option<String>,
    },
    /// 2xx whose body is **not** the response the client asked for — the
    /// relay-station failure mode where an upstream answers `200
    /// {"error": {...}}` because its *own* backend is down. Status alone
    /// cannot tell us this apart from success, so the shape has to.
    InvalidBody {
        status: u16,
        /// Fixed wording, never a quote of the upstream body: this string
        /// ends up in the `logs` row and in the error the client sees. The
        /// body itself goes to the debug capture instead.
        detail: &'static str,
        body: DebugCapture,
    },
    /// Connection / DNS / TLS / timeout failure — no HTTP status received.
    Transport(String),
}

/// A 2xx upstream response, either already read into memory or still live.
enum UpstreamBody {
    /// Non-streaming: body read and shape-validated by `try_upstream`.
    Buffered {
        status: StatusCode,
        content_type: Option<axum::http::HeaderValue>,
        bytes: Bytes,
    },
    /// Streaming: untouched from where `peek` left off, so the SSE pump can
    /// forward it without buffering. `prefix` holds whatever bytes the peek
    /// already consumed from `resp`; the pump seeds its line-scanning buffer
    /// with these so the very first frame isn't lost, but does not re-emit
    /// them to the client (the upstream already sent them; the gateway hasn't
    /// yet flushed headers).
    Live {
        resp: reqwest::Response,
        prefix: Vec<u8>,
    },
}

/// Does this JSON body look like a real response in `protocol`?
///
/// This is the check that catches a relay station answering `200` with an
/// error envelope. It is intentionally structural — one required field — and
/// not a full schema check: we only need to separate "a response the client
/// can parse" from "an error page dressed as a 200", and a stricter check
/// would start rejecting legitimate variants as providers add fields.
fn body_matches_protocol(bytes: &[u8], protocol: &str) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    if protocol == "anthropic" {
        v.get("type").and_then(|t| t.as_str()) == Some("message")
    } else {
        v.get("choices")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first())
            .map(|c| c.get("message").is_some())
            .unwrap_or(false)
    }
}

/// The log/error wording for a 2xx body that failed [`body_matches_protocol`].
fn invalid_body_detail(protocol: &str) -> &'static str {
    if protocol == "anthropic" {
        "HTTP 200 but body is not an Anthropic message"
    } else {
        "HTTP 200 but body is not an OpenAI completion"
    }
}

/// Build and dispatch one upstream request, classify the outcome.
///
/// Non-streaming 2xx responses are read into a [`DebugCapture`] and
/// shape-validated before being called a success — without this, a relay
/// station answering `200 {"error": ...}` short-circuits the candidate loop
/// and the client gets a body it can't parse, with no failover attempted.
///
/// Streaming 2xx can't be buffered (it's the response, live), so it gets the
/// weaker check instead: the `Content-Type` gate in [`is_json_content_type`].
/// The body there is one the client is already parsing incrementally, so the
/// damage is much smaller than a bad buffered body.
///
/// For `Http` the body is drained into a short [`read_error_detail`] note
/// (never forwarded anywhere) and for `Transport` there is nothing to read,
/// because the next target gets a fresh attempt either way.
async fn try_upstream(
    state: &AppState,
    cand: &Candidate,
    upstream_protocol: &str,
    body: Vec<u8>,
    headers: &HeaderMap,
    is_streaming: bool,
) -> UpstreamOutcome {
    let req = build_request(
        state,
        &cand.base_url,
        &cand.api_key,
        upstream_protocol,
        headers,
    )
    .body(body);
    match req.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            if (200..300).contains(&status) {
                if is_streaming {
                    return upstream_2xx_streaming_peek(resp, status).await;
                }
                return upstream_2xx_buffered(resp, status, upstream_protocol).await;
            }
            let retry_after_secs = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok());
            UpstreamOutcome::Http {
                status,
                retry_after_secs,
                detail: read_error_detail(resp).await,
            }
        }
        Err(e) => UpstreamOutcome::Transport(describe_transport_error(&e)),
    }
}

/// Read enough of a streaming response to know whether to commit it or fail
/// over to the next candidate.
///
/// The status line is already on the wire by the time the client is waiting,
/// so once we commit headers we cannot change our mind. To avoid handing the
/// client an SSE stream that is going to do nothing but emit an error event,
/// we look at the *first complete SSE frame* before flushing anything. Three
/// outcomes:
///
/// - The first frame is an error (`event: error`, or a `data:` payload shaped
///   like an error envelope) — return `InvalidBody` so the relay walks the
///   next candidate, identical to how a buffered 200-with-error-body is
///   handled.
/// - The first frame is content / `[DONE]` / blank-only — return `Live` with
///   the peeked bytes as a prefix; the pump drains them into its line buffer
///   before forwarding later chunks.
/// - The peek limit is hit without a frame — return `Live` with whatever was
///   buffered. Some providers batch their first event with the body and would
///   never trip a fast check; missing a slow first frame is less wrong than
///   burning the request because of it.
///
/// The limit is 8 KiB / 1.5 s. Most providers emit `message_start` or the
/// first chunk within milliseconds; anything slower is unusual enough that
/// failing over on a guess is more wrong than right.
async fn upstream_2xx_streaming_peek(mut resp: reqwest::Response, status: u16) -> UpstreamOutcome {
    if resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(is_json_content_type)
        .unwrap_or(false)
    {
        // Kept identical to the pre-peek behaviour: a stream request that
        // got back a complete JSON document instead of an event stream is
        // always wrong, with no need to wait for a frame.
        return UpstreamOutcome::InvalidBody {
            status,
            detail: "HTTP 200 but a streaming request got a JSON response, not an event stream",
            body: DebugCapture::new(),
        };
    }
    let mut buf: Vec<u8> = Vec::new();
    let capture = DebugCapture::new();
    const PEEK_MAX: usize = 8 * 1024;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(1500);
    loop {
        if buf.len() >= PEEK_MAX {
            break;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, resp.chunk()).await {
            Ok(Ok(Some(bytes))) => {
                capture.push(&bytes);
                buf.extend_from_slice(&bytes);
                if let Some(end) = sse_frame_end(&buf) {
                    let frame = &buf[..end];
                    let mut is_error = false;
                    for raw in frame.split(|b| *b == b'\n') {
                        let line = std::str::from_utf8(raw).unwrap_or("");
                        if sse_line_error(line.trim_end_matches('\r')).is_some() {
                            is_error = true;
                            break;
                        }
                    }
                    if is_error {
                        return UpstreamOutcome::InvalidBody {
                            status,
                            // Fixed wording; the upstream's own message is in
                            // the capture and the breaker reason.
                            detail: "HTTP 200 but stream opened with an upstream error event",
                            body: capture,
                        };
                    }
                    break;
                }
            }
            Ok(Ok(None)) | Ok(Err(_)) | Err(_) => break,
        }
    }
    UpstreamOutcome::Ok(UpstreamBody::Live { resp, prefix: buf })
}

/// Locate the byte index just past the end of the first complete SSE frame
/// in `buf`. A frame ends at `\n\n` or `\r\n\r\n`; everything after is the
/// next frame (or a partial one this function ignores). Returns `None` when
/// `buf` does not yet contain a full frame boundary, which the caller treats
/// as "read more".
fn sse_frame_end(buf: &[u8]) -> Option<usize> {
    if let Some(pos) = find_subsequence(buf, b"\n\n") {
        return Some(pos + 2);
    }
    find_subsequence(buf, b"\r\n\r\n").map(|pos| pos + 4)
}

/// Small, dependency-free subsequence search. `memchr` would be faster but
/// isn't worth a new dep for two calls per stream.
fn find_subsequence(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return if needle.is_empty() { Some(0) } else { None };
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// A 2xx on a non-streaming request: read it, then check it's actually a
/// response in the protocol we asked for.
async fn upstream_2xx_buffered(
    resp: reqwest::Response,
    status: u16,
    upstream_protocol: &str,
) -> UpstreamOutcome {
    let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).cloned();
    let capture = DebugCapture::new();
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            // A body that won't finish arriving is a transport failure wearing
            // a 200 status line; classify it as one.
            return UpstreamOutcome::Transport(format!("upstream body read failed: {e}"));
        }
    };
    capture.push(&bytes);
    if !body_matches_protocol(&bytes, upstream_protocol) {
        return UpstreamOutcome::InvalidBody {
            status,
            detail: invalid_body_detail(upstream_protocol),
            body: capture,
        };
    }
    UpstreamOutcome::Ok(UpstreamBody::Buffered {
        status: StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
        content_type,
        bytes,
    })
}

/// Does this content-type value denote a JSON document? Tolerates the
/// `application/json; charset=utf-8` form and `application/problem+json`.
fn is_json_content_type(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.starts_with("application/json") || value.ends_with("+json")
}

/// Turn a reqwest failure into a one-line cause for the breaker panel.
///
/// reqwest's own `Display` only says `error sending request for url (…)` —
/// the actionable part (DNS vs TLS vs refused vs timeout) lives further down
/// the `source()` chain, which is exactly what an admin staring at a tripped
/// breaker needs. So we classify on the well-known `is_*` predicates first and
/// fall back to the deepest `source()` message, skipping reqwest's own URL
/// boilerplate.
///
/// Shared with the background probe (`breaker_probe.rs`) so a probe and a
/// user request describe the same failure the same way.
pub(crate) fn describe_transport_error(e: &reqwest::Error) -> String {
    // Order matters: `is_timeout` and `is_connect` are both true for a
    // connect-timeout, and "connect" alone would hide the timeout.
    let kind = if e.is_timeout() {
        "timeout".to_string()
    } else if e.is_connect() {
        // reqwest flattens DNS / TLS / refused into `is_connect`, so dig
        // into the chain for the specific one where we can.
        let chain = source_chain(e);
        if chain.contains("dns error") || chain.contains("name resolution") {
            "dns".to_string()
        } else if chain.contains("certificate") || chain.contains("tls") {
            "tls".to_string()
        } else if chain.contains("Connection refused") {
            "refused".to_string()
        } else {
            "connect".to_string()
        }
    } else if e.is_request() {
        "request".to_string()
    } else if e.is_body() || e.is_decode() {
        "body".to_string()
    } else {
        // Unknown shape — surface the most specific message we can find.
        deepest_source(e).unwrap_or_else(|| e.to_string())
    };
    format!("transport: {kind}")
}

/// The full `source()` chain as one lowercased string, for substring probes.
fn source_chain(e: &dyn std::error::Error) -> String {
    let mut s = String::new();
    let mut cur = e.source();
    while let Some(src) = cur {
        s.push_str(&src.to_string());
        s.push(' ');
        cur = src.source();
    }
    s
}

/// The innermost `source()` message — the root cause rather than the wrapper.
fn deepest_source(e: &dyn std::error::Error) -> Option<String> {
    let mut last = None;
    let mut cur = e.source();
    while let Some(src) = cur {
        last = Some(src.to_string());
        cur = src.source();
    }
    last
}

/// Cap on how much of a failed upstream's body we read. Error payloads are
/// small in practice; the cap just stops a misbehaving upstream from making
/// the gateway buffer an unbounded body it is about to discard anyway.
const ERROR_DETAIL_MAX: usize = 4096;

/// Read a failed response's body and pull a human-readable cause out of it.
///
/// Best-effort: any read error yields `None` rather than propagating, since
/// the status code alone is already a usable fallback and the caller is on
/// its way to the next candidate anyway. Only the first
/// [`ERROR_DETAIL_MAX`] bytes are consumed — `extract_error_msg` truncates
/// its own output, so a longer body buys nothing.
///
/// `pub(crate)` because the background probe drains a failed response the
/// same way, so both paths describe a failure identically.
pub(crate) async fn read_error_detail(resp: reqwest::Response) -> Option<String> {
    use futures_util::StreamExt;
    let mut buf: Vec<u8> = Vec::new();
    let mut body = resp.bytes_stream();
    while let Some(chunk) = body.next().await {
        match chunk {
            Ok(bytes) => {
                let room = ERROR_DETAIL_MAX.saturating_sub(buf.len());
                if room == 0 {
                    break;
                }
                buf.extend_from_slice(&bytes[..bytes.len().min(room)]);
            }
            Err(_) => return None,
        }
    }
    let raw = String::from_utf8_lossy(&buf);
    let msg = admin::extract_error_msg(&raw);
    (!msg.is_empty()).then_some(msg)
}

/// Feed a non-2xx HTTP status into the breaker. 429 increments the
/// `retriable_429_count` so the final-status decision can choose 429 over
/// 502 when every failure was a rate-limit. The breaker itself no longer
/// cares about Retry-After — every failure is `Outcome::Failure` and
/// trips at the same threshold.
///
/// 400 / 422 are deliberately exempt: those are typically request-body
/// bugs (gateway-side or client-side) and tripping the breaker would hide
/// routing errors from the admin instead of surfacing them in logs.
/// Everything else — 5xx, 408, 429, and every other 4xx (including 402
/// out-of-credit, 404 model-gone, 405 protocol-mismatch) — is treated
/// as an upstream health signal and trips the breaker for the next
/// `base_delay` seconds.
///
/// Side effect: appends a `tripped` / `re-tripped` history row when the
/// breaker actually changes state (fire-and-forget, never blocks).
async fn record_outcome_in_breaker(
    state: &AppState,
    breaker_key: &str,
    code: u16,
    detail: Option<&str>,
    _retry_after_secs: Option<u64>,
    retriable_429_count: &mut usize,
) {
    if code == 429 {
        *retriable_429_count += 1;
    }
    if code != 400 && code != 422 {
        // Prefer the upstream's own wording so the panel says *why* it
        // tripped (`HTTP 429: Rate limit exceeded`) instead of a bare code;
        // `clip_reason` inside the breaker trims an over-long message.
        let reason = match detail {
            Some(d) => format!("HTTP {code}: {d}"),
            None => format!("HTTP {code}"),
        };
        let transition = state
            .breaker
            .record(breaker_key, Outcome::Failure(reason))
            .await;
        let event_kind = match transition.kind {
            breaker::TransitionKind::Inserted => Some(BreakerEventKind::Tripped),
            breaker::TransitionKind::Updated => Some(BreakerEventKind::ReTripped),
            breaker::TransitionKind::Removed | breaker::TransitionKind::None => None,
        };
        if let Some(event) = event_kind {
            // Splits on the first `|`, so a model name containing `|`
            // round-trips correctly (see `breaker::split_key_owned` test).
            let (channel, model) = breaker::split_key_owned(breaker_key);
            record_breaker_event(
                &state.pool,
                BreakerEventRow {
                    channel_name: channel,
                    target_model: model,
                    event,
                    reason: transition.reason,
                    backoff_secs: transition.backoff_secs,
                },
            );
        }
    }
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
    /// Shared with the relay that owns this stream, so the Drop-driven
    /// capture writes the live (not frozen) debug flag value.
    state: Arc<AppState>,
    entry: LogEntry,
    /// `channel|model` for the hop that opened this stream. Needed because a
    /// stream's verdict can only be settled when the stream *ends* — long
    /// after `relay()` returned and gave up ownership of the candidate walk.
    breaker_key: String,
}

impl StreamLog {
    /// Finalize the log row and write the captured upstream bytes to disk
    /// when there's a reason to (`should_capture`). Usage is supplied by the
    /// caller (Drop impl of LogOnEnd).
    async fn spawn_inline(
        mut self,
        usage: Option<convert::Usage>,
        capture: DebugCapture,
        stream_error: Option<String>,
    ) {
        // A 200 whose body turned out to be an error is a failure, and it has
        // to be recorded as one: the log page otherwise shows a green 200 with
        // zero tokens, and the breaker records Success and never backs off, so
        // a dead provider keeps getting picked. The status stays 200 because
        // that *is* what the client received — the status line was committed
        // before a single body byte existed.
        //
        // The mutation has to touch both the summary `winner` field and the
        // winner's clone at the tail of `attempts`: `log_request` reads the
        // `ok` flag from each `attempts` row independently (the per-attempt
        // `log_attempts` table is what the list page filters on), and the two
        // start out as separate clones of the same Attempt.
        if let Some(reason) = stream_error {
            if let Some(winner) = self.entry.winner.as_mut() {
                winner.ok = false;
                winner.error = STREAM_ERROR_DETAIL.to_string();
            }
            if let Some(last) = self.entry.attempts.last_mut() {
                last.ok = false;
                last.error = STREAM_ERROR_DETAIL.to_string();
            }
            self.state
                .breaker
                .record(&self.breaker_key, Outcome::Failure(reason))
                .await;
        }
        if let Some(winner) = self.entry.winner.as_mut() {
            winner.usage = usage;
        }
        let ok = self.entry.winner.as_ref().map(|w| w.ok).unwrap_or(false);
        let log_id = log_request(&self.pool, &self.entry).await;
        if should_capture(&self.state, ok) {
            if let Some(id) = log_id {
                write_debug_log(&self.pool, id, &capture).await;
            }
        }
    }
}

/// Whether debug logging is currently enabled. Read on every request so
/// the runtime toggle takes effect without a restart — the bug the static
/// version had was it being snapshotted into AppState at boot. The
/// `set_debug_logging` helper below keeps the public API stable for callers
/// that only have a bool and the AppState alongside it.
fn state_debug_logging(state: &AppState) -> bool {
    state
        .debug_logging
        .load(std::sync::atomic::Ordering::Relaxed)
}

/// Do we need to write a capture file for a request that ended this way?
///
/// Always for failures. That's the whole point: the switch is a *diagnostic
/// for healthy traffic*, but a request that failed is exactly the one whose
/// upstream body nobody can reconstruct later, and the admin looking at a red
/// row in the log list shouldn't have to wonder whether the switch happened
/// to be on when it happened. Failures are rare enough that the disk cost is
/// bounded by the error rate rather than by traffic.
///
/// The switch still means "capture everything", successes included.
fn should_capture(state: &AppState, ok: bool) -> bool {
    !ok || state_debug_logging(state)
}

/// Set the per-state debug_logging flag. Called at startup and by the admin
/// settings handler; the change is immediate because every relay call reads
/// the live value.
pub fn set_debug_logging(state: &AppState, v: bool) {
    state
        .debug_logging
        .store(v, std::sync::atomic::Ordering::Relaxed);
}

/// Wraps a byte stream and spawns a log task when the wrapper is dropped.
/// Drop fires on three paths:
///   1. upstream ended cleanly → Poll::Ready(None)
///   2. axum's response body finished streaming to the client (success)
///   3. the client disconnected mid-stream → upstream gets cancelled, the
///      body_stream future is dropped
///
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
    /// The upstream response body, accumulated as it streams past. Read at
    /// Drop and forwarded to `StreamLog` for the debug capture file.
    capture: DebugCapture,
    /// Set by the pump when an SSE line announces an upstream error. The
    /// stream is still forwarded verbatim — an in-stream error is a legitimate
    /// terminal event that the client's protocol defines — but this is what
    /// makes the hop count as a failure once it ends.
    stream_error: Arc<Mutex<Option<String>>>,
}

impl<S> LogOnEnd<S> {
    fn wrap(
        inner: S,
        log: StreamLog,
        usage: Arc<Mutex<Option<convert::Usage>>>,
        capture: DebugCapture,
        stream_error: Arc<Mutex<Option<String>>>,
    ) -> Self {
        Self {
            inner,
            log: Some(log),
            usage,
            converter_usage: None,
            capture,
            stream_error,
        }
    }
    fn wrap_with_converter(
        inner: S,
        log: StreamLog,
        converter: Arc<Mutex<Box<dyn SseConverter>>>,
        capture: DebugCapture,
        stream_error: Arc<Mutex<Option<String>>>,
    ) -> Self {
        Self {
            inner,
            log: Some(log),
            usage: Arc::new(Mutex::new(None)),
            converter_usage: Some(converter),
            capture,
            stream_error,
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
                *self.usage.lock().unwrap()
            };
            let capture = self.capture.clone();
            let stream_error = self.stream_error.lock().unwrap().clone();
            tokio::spawn(async move {
                log.spawn_inline(usage, capture, stream_error).await;
            });
        }
    }
}

/// Pull the token breakdown out of one SSE `data:` payload. Implemented
/// in `convert.rs` since that's where the OpenAI/Anthropic field-name
/// knowledge already lives; re-exported here for the passthrough path.
/// Capture the first message-bearing detection while letting the placeholder
/// be upgraded later. `event: error` alone carries no message, so locking it
/// in on first sight would discard the `data:` line right behind it.
fn note_stream_error(slot: &std::sync::Mutex<Option<String>>, reason: String) {
    let mut slot = slot.lock().unwrap();
    match slot.as_deref() {
        None => *slot = Some(reason),
        Some(STREAM_ERROR_DETAIL) if reason != STREAM_ERROR_DETAIL => *slot = Some(reason),
        _ => {}
    }
}

fn usage_from_sse_payload(payload: &str) -> Option<convert::Usage> {
    convert::usage_from_sse_payload(payload)
}

/// The wording written to the log row when a stream carried an error event.
///
/// Fixed, and never a quote of the upstream text — same rule as
/// [`UpstreamOutcome::InvalidBody`]: the row stays a stable label while the
/// upstream's own wording goes to the breaker reason and the raw bytes to the
/// capture file.
const STREAM_ERROR_DETAIL: &str = "upstream error event inside a 200 stream";

/// Does this SSE line announce that the upstream gave up?
///
/// Returns the upstream's own message when it has one, for the breaker panel.
/// Streams report failure two different ways and both have to be caught:
///
/// - Anthropic (and relays copying it): an `event: error` line, whose `data:`
///   payload is `{"type":"error","error":{...}}`.
/// - OpenAI-compatible relays: no `event:` line at all, just a `data:` payload
///   carrying an `error` object.
///
/// The test is deliberately structural and anchored to the *top level*: a
/// model whose own text mentions errors arrives as
/// `{"type":"content_block_delta","delta":{"text":"error: …"}}`, which must
/// not be mistaken for one. That's why "error" as a substring never matches —
/// only `type == "error"` or an actual `error` key does. The `error` half is
/// deliberately loose about the value's shape: relays emit the string
/// `"overloaded"` there as often as an object, and no successful frame in
/// either protocol carries the key at all.
///
/// `Some(…)` is idempotent per stream: the caller keeps the first hit, so a
/// provider that emits one error frame can't overwrite a more specific one.
fn sse_line_error(line: &str) -> Option<String> {
    let trimmed = line.trim_end_matches('\r');
    if trimmed.strip_prefix("event:").map(str::trim) == Some("error") {
        return Some(STREAM_ERROR_DETAIL.to_string());
    }
    let payload = trimmed.strip_prefix("data:")?.trim();
    let v: Value = serde_json::from_str(payload).ok()?;
    let is_error = v.get("type").and_then(|t| t.as_str()) == Some("error")
        || v.get("error").is_some_and(|e| !e.is_null());
    if !is_error {
        return None;
    }
    Some(
        v.pointer("/error/message")
            .and_then(|m| m.as_str())
            .filter(|m| !m.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| STREAM_ERROR_DETAIL.to_string()),
    )
}

/// Forward an upstream SSE byte stream to the client verbatim, but parse
/// `data:` lines on the side so we can record token usage when the stream
/// ends. Same drop-cancels-upstream property as the converted stream.
fn passthrough_stream(
    prefix: Vec<u8>,
    resp: reqwest::Response,
    status: StatusCode,
    content_type: Option<axum::http::HeaderValue>,
    log: StreamLog,
    capture: DebugCapture,
) -> Response {
    let usage: Arc<Mutex<Option<convert::Usage>>> = Arc::new(Mutex::new(None));
    let usage_for_drop = Arc::clone(&usage);
    let stream_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let err_for_pump = Arc::clone(&stream_error);
    // The first unfold iteration is dedicated to forwarding whatever bytes
    // the peek already consumed from `resp`. Without this, the client would
    // see only the chunks that arrive *after* the prefix was buffered (in
    // tests where the upstream serves the whole body in a single chunk, the
    // client would see an empty stream). After the prefix is flushed, the
    // loop falls into the normal chunk-by-chunk path.
    let body_stream = stream::unfold(
        (
            resp,
            prefix,
            Arc::clone(&usage),
            capture.clone(),
            err_for_pump,
        ),
        |mut st| async move {
            let (resp, buf, usage_ref, capture, err_ref) =
                (&mut st.0, &mut st.1, &mut st.2, &st.3, &st.4);
            if !buf.is_empty() {
                let prefix_bytes = std::mem::take(buf);
                capture.push(&prefix_bytes);
                // The prefix was assembled during the peek, so we have to
                // re-scan its lines for both usage and the in-stream error
                // detector; passthrough's normal chunk loop wouldn't see
                // these bytes again.
                let mut scan = prefix_bytes.clone();
                while let Some(pos) = scan.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = scan.drain(..=pos).collect();
                    let trimmed = std::str::from_utf8(&line)
                        .unwrap_or("")
                        .trim_end_matches('\r');
                    if let Some(reason) = sse_line_error(trimmed) {
                        note_stream_error(err_ref, reason);
                    }
                    if let Some(payload) = trimmed.strip_prefix("data:") {
                        if let Some(u) = usage_from_sse_payload(payload.trim()) {
                            *usage_ref.lock().unwrap() = Some(u);
                        }
                    }
                }
                return Some((Ok::<Bytes, reqwest::Error>(Bytes::from(prefix_bytes)), st));
            }
            match resp.chunk().await {
                Ok(Some(bytes)) => {
                    // Capture the raw upstream bytes for the debug log, then
                    // copy into buf for side-effect usage extraction before
                    // forwarding the original chunk to the client untouched.
                    // Copying (not moving) lets us hand `bytes` straight to
                    // axum while keeping the parse buffer authoritative for
                    // line scanning.
                    capture.push(&bytes);
                    buf.extend_from_slice(&bytes);
                    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=pos).collect();
                        let line_str = std::str::from_utf8(&line).unwrap_or("");
                        let trimmed = line_str.trim_end_matches('\r');
                        if let Some(reason) = sse_line_error(trimmed) {
                            note_stream_error(err_ref, reason);
                        }
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
            capture,
            stream_error,
        )))
        .unwrap_or_else(|_| {
            error_response("openai", StatusCode::BAD_GATEWAY, "body build failed", None)
        })
}

/// Convert an upstream SSE byte stream to the client's protocol, line by
/// line, via an `SseConverter`. Same drop-cancels-upstream property as the
/// passthrough stream. The converter is shared via `Arc<Mutex<>>` so the
/// final token usage can be read out from `converter.usage()` when the
/// stream ends — the converter already tracks usage as a side-effect of
/// translating each chunk, so we don't need a second SSE parser here.
fn converted_stream(
    prefix: Vec<u8>,
    resp: reqwest::Response,
    conv: Box<dyn SseConverter>,
    log: StreamLog,
    capture: DebugCapture,
) -> Response {
    let conv: Arc<Mutex<Box<dyn SseConverter>>> = Arc::new(Mutex::new(conv));
    let stream_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let mut response = Response::builder().status(StatusCode::OK);
    if let Some(h) = response.headers_mut() {
        h.insert(
            axum::http::header::CONTENT_TYPE,
            "text/event-stream".parse().unwrap(),
        );
    }
    let state = (
        resp,
        String::from_utf8_lossy(&prefix).into_owned(),
        Arc::clone(&conv),
        false,
        capture.clone(),
        stream_error.clone(),
    );
    let body_stream = stream::unfold(state, |mut st| async move {
        let (resp, buf, conv_ref, done, capture, err_ref) =
            (&mut st.0, &mut st.1, &mut st.2, &mut st.3, &st.4, &st.5);
        loop {
            if *done {
                return None;
            }
            if let Some(pos) = buf.find('\n') {
                let line = buf[..pos].trim_end_matches('\r').to_string();
                buf.drain(..=pos);
                // Read the error off the *raw upstream* line, before the
                // converter sees it. The converter drops event types it
                // doesn't model, so an error frame would otherwise leave no
                // trace anywhere and this hop would log as a clean success.
                if let Some(reason) = sse_line_error(&line) {
                    note_stream_error(err_ref, reason);
                }
                if let Some(payload) = line.strip_prefix("data:") {
                    let payload = payload.trim();
                    let events = if payload == "[DONE]" {
                        *done = true;
                        conv_ref.lock().unwrap().finish()
                    } else {
                        conv_ref.lock().unwrap().on_data(payload)
                    };
                    if !events.is_empty() {
                        return Some((
                            Ok::<Bytes, reqwest::Error>(Bytes::from(events.concat())),
                            st,
                        ));
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
                    // Capture what the *upstream* sent, not the translated
                    // events — the same thing both stream paths record, so
                    // `resp.json` always answers "what did the provider
                    // actually reply" regardless of translation.
                    capture.push(&bytes);
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
            capture,
            stream_error,
        )))
        .unwrap_or_else(|_| {
            error_response("openai", StatusCode::BAD_GATEWAY, "body build failed", None)
        })
}

/// Everything about the client request that stays constant across candidates.
/// Threading these as one struct keeps the signatures at a sane arity and
/// makes it obvious which values the failover loop may not vary per hop.
struct RelayCtx<'a> {
    state: &'a AppState,
    state_arc: Arc<AppState>,
    token_name: &'a str,
    /// The model the client asked for.
    request_model: &'a str,
    /// The protocol the client speaks — not necessarily the upstream's.
    protocol: &'a str,
    streaming: bool,
    start: Instant,
    client_info: ClientInfo,
}

/// Convert an upstream response into the client response, returning the
/// `Attempt` that produced it so the caller can record it as the winning hop
/// of this request. This function does not log — logging is the caller's job,
/// because only the caller knows about the hops that failed before this one.
///
/// **Only called on a validated 2xx** — the main loop falls through to the
/// next target on every other outcome, so the streaming and conversion paths
/// here can assume success.
///
/// Streaming requests pass through unchanged; non-streaming responses arrive
/// already buffered. `model` is the post-mapping model sent upstream. `start`
/// is when the relay started, so we can record how long the upstream took to
/// first respond (TTFB); on stream replay it's frozen at the transition.
async fn respond_from_upstream(
    ctx: &RelayCtx<'_>,
    model: &str,
    cand: &Candidate,
    body: UpstreamBody,
    attempts: Vec<Attempt>,
) -> (Response, Attempt) {
    let state = ctx.state;
    // Latency up to "we got the upstream's response headers". For streams
    // this is the TTFB, which is what people usually want; for buffered
    // responses this is the whole round-trip.
    let latency_ms = ctx.start.elapsed().as_millis() as i64;
    let entry_of = |winner: Attempt, attempts: Vec<Attempt>| LogEntry {
        token_name: ctx.token_name.to_string(),
        request_model: ctx.request_model.to_string(),
        protocol: ctx.protocol.to_string(),
        streaming: ctx.streaming,
        winner: Some(winner.clone()),
        attempts: push(attempts, winner),
        client_ip: ctx.client_info.ip.clone(),
        user_agent: ctx.client_info.user_agent.clone(),
    };

    let (status, content_type, bytes) = match body {
        UpstreamBody::Buffered {
            status,
            content_type,
            bytes,
        } => (status, content_type, bytes),
        UpstreamBody::Live { resp, prefix } => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let content_type = resp.headers().get("content-type").cloned();
            // This hop ends the request either way — the client gets its
            // response here, whether it succeeded or not.
            let attempt = Attempt::new(model, cand, status.as_u16() as i64, "", latency_ms, true);
            let entry = entry_of(attempt.clone(), attempts);
            let log = StreamLog {
                pool: state.pool.clone(),
                state: ctx.state_arc.clone(),
                entry,
                breaker_key: breaker::breaker_key(&cand.name, model),
            };
            // The bytes the peek already consumed get captured here so the
            // debug log shows the full stream the upstream actually sent, with
            // no gap at the start. The pump will *not* re-emit them; only
            // chunks arriving after the prefix reach the client.
            let capture = DebugCapture::new();
            capture.push(&prefix);
            let response = match cand.convert {
                ConvertMode::None => {
                    passthrough_stream(prefix, resp, status, content_type, log, capture.clone())
                }
                ConvertMode::ToOpenAI => {
                    let conv = convert::OpenAiToAnthropicStream::new(model);
                    converted_stream(prefix, resp, Box::new(conv), log, capture.clone())
                }
                ConvertMode::ToAnthropic => {
                    let conv = convert::AnthropicToOpenAiStream::new(model);
                    converted_stream(prefix, resp, Box::new(conv), log, capture.clone())
                }
            };
            return (response, attempt);
        }
    };

    let attempt = Attempt::new(model, cand, status.as_u16() as i64, "", latency_ms, true)
        .with_usage(parse_usage(&bytes));
    // Translate the buffered body when this hop crosses protocols. The body
    // has already been validated against the *upstream's* protocol, so a
    // parse failure here is only reachable if a provider returns something
    // unparseable — pass the bytes through rather than inventing an error.
    let out_bytes = if cand.convert != ConvertMode::None {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => {
                let converted = match cand.convert {
                    ConvertMode::ToOpenAI => convert::openai_resp_to_anthropic(&v, model),
                    ConvertMode::ToAnthropic => convert::anthropic_resp_to_openai(&v, model),
                    ConvertMode::None => unreachable!(),
                };
                serde_json::to_vec(&converted).unwrap_or_else(|_| bytes.to_vec())
            }
            Err(_) => bytes.to_vec(),
        }
    } else {
        bytes.to_vec()
    };
    let entry = entry_of(attempt.clone(), attempts);
    let ok = attempt.ok;
    let log_id = log_request(&state.pool, &entry).await;
    if should_capture(state, ok) {
        let capture = DebugCapture::new();
        capture.push(&bytes);
        if let Some(id) = log_id {
            write_debug_log(&state.pool, id, &capture).await;
        }
    }
    let mut builder = Response::builder().status(status);
    if let Some(ct) = content_type {
        if let Some(h) = builder.headers_mut() {
            h.insert(axum::http::header::CONTENT_TYPE, ct);
        }
    }
    let resp = builder.body(Body::from(out_bytes)).unwrap_or_else(|_| {
        error_response("openai", StatusCode::BAD_GATEWAY, "body build failed", None)
    });
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
    state_arc: Arc<AppState>,
    headers: &HeaderMap,
    body: axum::body::Bytes,
    protocol: &str,
    direct_ip: Option<std::net::SocketAddr>,
) -> Response {
    let state = &*state_arc;
    // 1. internal token auth
    let key = match extract_token(headers) {
        Some(k) => k,
        None => {
            return error_response(
                protocol,
                StatusCode::UNAUTHORIZED,
                "missing bearer token",
                None,
            )
        }
    };
    let token_name = match auth_token(state, &key).await {
        Ok(n) => n,
        Err((s, msg)) => return error_response(protocol, s, msg, None),
    };
    let client_info = extract_client_info(headers, direct_ip);

    // 2. parse body to find model + streaming flag
    let req_json: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return error_response(protocol, StatusCode::BAD_REQUEST, "invalid JSON body", None)
        }
    };
    let model = match req_json.get("model").and_then(|m| m.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return error_response(
                protocol,
                StatusCode::BAD_REQUEST,
                "missing model in request",
                None,
            )
        }
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
    let mut relay_retry_after: Option<String> = None;
    let mut attempted = 0usize;
    // Hops that actually reached an upstream, i.e. `attempted` minus the ones
    // the breaker skipped. The final-status decision below compares the
    // 429/transport tallies against this rather than `attempted`: a skip is
    // not evidence about *how* the upstreams failed, so letting it dilute the
    // denominator would turn "everything that answered was rate-limited" into
    // a 502 and drop the `Retry-After` the client needs to back off.
    let mut upstream_attempts = 0usize;
    let mut retriable_429_count = 0usize;
    let mut transport_err_count = 0usize;
    // Every upstream attempt so far, in order. Recorded as child rows of the
    // single `logs` row this request produces. The winning attempt — the
    // 2xx response that gets returned to the client — is pushed by
    // `respond_from_upstream`; failed hops are pushed inline below.
    let mut attempts: Vec<Attempt> = Vec::new();
    // The upstream body from the most recent unusable-2xx hop, if any. Held
    // rather than written immediately so only the final failure is persisted.
    let mut last_capture: Option<DebugCapture> = None;
    for (pin_channel, target_model) in &targets {
        let candidates = if pin_channel.is_empty() {
            match candidate_channels(state, target_model, protocol).await {
                Ok(c) => c,
                Err(s) => return error_response(protocol, s, "internal error", None),
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
            all_errors.push(format!(
                "no enabled channel provides model `{}`",
                target_model
            ));
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
                ConvertMode::ToOpenAI => convert::anthropic_req_to_openai(&req_json, target_model),
                ConvertMode::ToAnthropic => {
                    convert::openai_req_to_anthropic(&req_json, target_model)
                }
            };
            let target_body = serde_json::to_vec(&body_json).unwrap_or_else(|_| body.to_vec());
            // Breaker gate: skip this (channel, model) without burning a
            // network call while it's OPEN. Recorded as a hop so the log
            // page shows the request tried it (admin still sees what was
            // skipped) but `all_errors` is *not* updated — the failure is
            // not a "real" upstream failure and shouldn't bias the
            // final-status decision (block at 1652 area in old code).
            let breaker_key = breaker::breaker_key(&cand.name, target_model);
            if !state.breaker.allow(&breaker_key).await {
                attempted += 1;
                attempts.push(
                    Attempt::new(
                        target_model,
                        cand,
                        0,
                        "circuit breaker open",
                        relay_start.elapsed().as_millis() as i64,
                        false,
                    )
                    .skipped(),
                );
                continue;
            }
            attempted += 1;
            upstream_attempts += 1;
            let outcome = try_upstream(
                state,
                cand,
                upstream_protocol,
                target_body,
                headers,
                is_streaming,
            )
            .await;
            match outcome {
                UpstreamOutcome::Ok(body) => {
                    state.breaker.record(&breaker_key, Outcome::Success).await;
                    return respond_from_upstream(
                        &RelayCtx {
                            state,
                            state_arc: state_arc.clone(),
                            token_name: &token_name,
                            request_model: &model,
                            protocol,
                            streaming: is_streaming,
                            start: relay_start,
                            client_info: client_info.clone(),
                        },
                        target_model,
                        cand,
                        body,
                        attempts,
                    )
                    .await
                    .0;
                }
                UpstreamOutcome::Http {
                    status: code,
                    retry_after_secs,
                    detail,
                } => {
                    let elapsed = relay_start.elapsed().as_millis() as i64;
                    // Stays a bare status code: this string is both the log
                    // row and part of the error body the client eventually
                    // sees. The richer upstream wording goes to the breaker
                    // panel only — see `record_outcome_in_breaker`.
                    let err_msg = format!("HTTP {code}");
                    attempts.push(Attempt::new(
                        target_model,
                        cand,
                        code as i64,
                        &err_msg,
                        elapsed,
                        false,
                    ));
                    all_errors.push(format!("{} ({}) -> {}", cand.name, target_model, err_msg));
                    // First non-empty Retry-After wins for the client
                    // pass-through on the eventual 429 response.
                    if relay_retry_after.is_none() {
                        relay_retry_after = retry_after_secs.map(|s| s.to_string());
                    }
                    record_outcome_in_breaker(
                        state,
                        &breaker_key,
                        code,
                        detail.as_deref(),
                        retry_after_secs,
                        &mut retriable_429_count,
                    )
                    .await;
                    // Fall through to the next candidate / next target.
                }
                UpstreamOutcome::InvalidBody {
                    status: code,
                    detail,
                    body,
                } => {
                    let elapsed = relay_start.elapsed().as_millis() as i64;
                    attempts.push(Attempt::new(
                        target_model,
                        cand,
                        code as i64,
                        detail,
                        elapsed,
                        false,
                    ));
                    all_errors.push(format!("{} ({}) -> {}", cand.name, target_model, detail));
                    // Not a 429 and not a transport error, so neither counter
                    // moves: the final-status decision lands on 502, which is
                    // right — we got a response, it just wasn't usable.
                    state
                        .breaker
                        .record(
                            &breaker_key,
                            Outcome::Failure(format!("HTTP {code}: {detail}")),
                        )
                        .await;
                    // Keep the offending body for the log detail page. Only
                    // the last one is retained, so a long failover chain
                    // writes one file, not one per hop.
                    last_capture = Some(body);
                    // Fall through to the next candidate / next target.
                }
                UpstreamOutcome::Transport(err_msg) => {
                    let elapsed = relay_start.elapsed().as_millis() as i64;
                    attempts.push(Attempt::new(
                        target_model,
                        cand,
                        -1,
                        &err_msg,
                        elapsed,
                        false,
                    ));
                    all_errors.push(format!("{} ({}): {}", cand.name, target_model, err_msg));
                    transport_err_count += 1;
                    state
                        .breaker
                        .record(&breaker_key, Outcome::Failure(err_msg.clone()))
                        .await;
                    // Fall through to the next candidate / next target.
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
                upstream_model: targets.first().map(|(_, m)| m.clone()).unwrap_or_default(),
                channel_name: String::new(),
                status: StatusCode::NOT_FOUND.as_u16() as i64,
                error: err.clone(),
                latency_ms: relay_start.elapsed().as_millis() as i64,
                convert: ConvertMode::None,
                ok: false,
                skipped: false,
                usage: None,
            }),
            attempts: Vec::new(),
            client_ip: client_info.ip.clone(),
            user_agent: client_info.user_agent.clone(),
        };
        let _ = log_request(&state.pool, &entry).await;
        return error_response(protocol, StatusCode::NOT_FOUND, &err, None);
    }
    // Pick the most informative status code:
    //   - nothing reached an upstream (every hop was a breaker skip) -> 502;
    //     there is no tally to read, and 429/504 would both be invented
    //   - all retriable failures were 429 -> 429 (so SDKs that honor 429's
    //     Retry-After can back off correctly instead of blind exponential)
    //   - all failures were transport-level -> 504 (none of the upstreams
    //     even responded)
    //   - anything else (5xx mix, 429+5xx mix) -> 502 (gateway saw responses
    //     but couldn't serve the request)
    //
    // The denominators are `upstream_attempts`, not `attempted`: breaker skips
    // are recorded as hops but never contacted anyone, so counting them would
    // downgrade an otherwise-uniform 429 to a bare 502 and lose the
    // `Retry-After` pass-through below.
    let final_status = if upstream_attempts == 0 {
        StatusCode::BAD_GATEWAY
    } else if retriable_429_count == upstream_attempts {
        StatusCode::TOO_MANY_REQUESTS
    } else if transport_err_count == upstream_attempts {
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
        skipped: false,
        usage: None,
    };
    let entry = LogEntry {
        token_name,
        request_model: model,
        protocol: protocol.to_string(),
        streaming: is_streaming,
        winner: Some(all_failed),
        attempts,
        client_ip: client_info.ip.clone(),
        user_agent: client_info.user_agent.clone(),
    };
    let log_id = log_request(&state.pool, &entry).await;
    // Unconditional: a request that exhausted every candidate is exactly the
    // one an admin will want to investigate, and whether the debug switch was
    // on when it happened is not something they can retroactively know.
    if let Some(capture) = last_capture {
        if let Some(id) = log_id {
            write_debug_log(&state.pool, id, &capture).await;
        }
    }
    error_response(protocol, final_status, &err, relay_retry_after.as_deref())
}

/// POST /v1/chat/completions — OpenAI-compatible relay.
pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    OptionalConnectInfo(addr): OptionalConnectInfo,
    body: axum::body::Bytes,
) -> Response {
    relay(state, &headers, body, "openai", addr).await
}

/// POST /v1/messages — Anthropic Messages API relay.
pub async fn anthropic_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    OptionalConnectInfo(addr): OptionalConnectInfo,
    body: axum::body::Bytes,
) -> Response {
    relay(state, &headers, body, "anthropic", addr).await
}

/// GET /v1/models — list union of all enabled channel models.
pub async fn list_models(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let key = match extract_token(&headers) {
        Some(k) => k,
        None => {
            return error_response(
                "openai",
                StatusCode::UNAUTHORIZED,
                "missing bearer token",
                None,
            )
        }
    };
    if let Err((s, msg)) = auth_token(&state, &key).await {
        return error_response("openai", s, msg, None);
    }
    // only external channels serve relay traffic (see candidate_channels)
    let rows = match sqlx::query("SELECT models FROM channels WHERE enabled=1")
        .fetch_all(&state.pool)
        .await
    {
        Ok(r) => r,
        Err(_) => {
            return error_response(
                "openai",
                StatusCode::INTERNAL_SERVER_ERROR,
                "db error",
                None,
            )
        }
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
// ---- tests ----------------------------------------------------------------
//
// These cover the private request-entry helpers. They live in-file rather
// than in `tests/` because the whole point is to pin behaviour that the
// public HTTP surface can only reach indirectly, and because `extract_token`
// in particular is the single place every `/v1/*` call gets its credential
// from — a regression there is a gateway-wide auth failure, not a
// single-endpoint bug.

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::time::Duration;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        m
    }

    // ---------- describe_transport_error ----------

    /// Build a real reqwest error by actually attempting a connection to a
    /// closed port, so the classification is driven by the genuine error
    /// chain rather than a hand-rolled stand-in.
    async fn closed_port_error() -> reqwest::Error {
        reqwest::Client::new()
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .expect_err("port 1 must not accept connections")
    }

    #[tokio::test]
    async fn a_refused_connection_is_classified_rather_than_echoed() {
        let e = closed_port_error().await;
        let reason = describe_transport_error(&e);
        assert!(
            reason.starts_with("transport: "),
            "expected a `transport: <kind>` shape, got {reason:?}"
        );
        assert!(
            !reason.contains("error sending request"),
            "reqwest's URL boilerplate should be replaced, got {reason:?}"
        );
        assert!(
            !reason.contains("http://127.0.0.1:1"),
            "the URL adds nothing an admin doesn't already see, got {reason:?}"
        );
    }

    #[tokio::test]
    async fn a_timeout_wins_over_the_generic_connect_classification() {
        // `is_timeout` and `is_connect` are both true for a connect-timeout;
        // reporting "connect" would hide the more useful signal.
        let e = reqwest::Client::new()
            .get("http://10.255.255.1/")
            .timeout(Duration::from_millis(1))
            .send()
            .await
            .expect_err("a 1ms timeout must fire");
        assert!(
            e.is_timeout(),
            "test premise: expected a timeout, got {e:?}"
        );
        assert_eq!(describe_transport_error(&e), "transport: timeout");
    }

    #[test]
    fn the_root_cause_is_preferred_over_the_wrapper_message() {
        // A synthetic chain: an outer error whose `Display` says nothing
        // useful, wrapping one that names the actual cause.
        #[derive(Debug)]
        struct Wrapper(Inner);
        impl std::fmt::Display for Wrapper {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("wrapper said nothing helpful")
            }
        }
        impl std::error::Error for Wrapper {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        #[derive(Debug)]
        struct Inner;
        impl std::fmt::Display for Inner {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("the actual root cause")
            }
        }
        impl std::error::Error for Inner {}

        assert_eq!(
            deepest_source(&Wrapper(Inner)).as_deref(),
            Some("the actual root cause")
        );
        // A leaf error has no `source()` to offer.
        assert_eq!(deepest_source(&Inner), None);
    }

    // ---------- extract_token ----------

    #[test]
    fn an_openai_style_bearer_token_is_extracted() {
        assert_eq!(
            extract_token(&headers(&[("authorization", "Bearer sk-abc")])),
            Some("sk-abc".to_string())
        );
    }

    #[test]
    fn an_anthropic_style_api_key_is_extracted() {
        assert_eq!(
            extract_token(&headers(&[("x-api-key", "sk-abc")])),
            Some("sk-abc".to_string())
        );
    }

    #[test]
    fn x_api_key_is_the_fallback_when_authorization_is_absent() {
        // Claude Code and friends send `x-api-key` and may set an unrelated
        // Authorization; the explicit x-api-key must not be shadowed.
        assert_eq!(
            extract_token(&headers(&[
                ("authorization", "Bearer other"),
                ("x-api-key", "sk-abc")
            ])),
            Some("other".to_string())
        );
    }

    #[test]
    fn the_bearer_prefix_match_is_case_sensitive() {
        // RFC 7235 says the scheme is case-insensitive, so a client sending
        // `bearer` today gets no credential and a 401. Pinned so that
        // accepting the lowercase form later is a deliberate, visible change.
        assert_eq!(
            extract_token(&headers(&[("authorization", "bearer sk-abc")])),
            None
        );
    }

    #[test]
    fn a_request_with_no_credential_yields_none() {
        assert_eq!(extract_token(&headers(&[])), None);
    }

    #[test]
    fn a_bearer_with_an_empty_token_is_still_returned_and_rejected_downstream() {
        // Better to hand the empty string to the DB lookup (which rejects it)
        // than to skip the auth path entirely.
        assert_eq!(
            extract_token(&headers(&[("authorization", "Bearer ")])),
            Some(String::new())
        );
    }

    #[test]
    fn a_non_bearer_authorization_is_ignored() {
        assert_eq!(
            extract_token(&headers(&[("authorization", "Basic dXNlcjpwYXNz")])),
            None
        );
    }

    // ---------- extract_client_info ----------

    #[test]
    fn the_first_forwarded_for_hop_is_the_client() {
        // `X-Forwarded-For: client, proxy1, proxy2` — the leftmost entry is
        // the original client.
        let info = extract_client_info(
            &headers(&[("x-forwarded-for", "203.0.113.7, 10.0.0.1, 10.0.0.2")]),
            Some("10.0.0.9:1234".parse().unwrap()),
        );
        assert_eq!(info.ip, "203.0.113.7");
    }

    #[test]
    fn the_direct_peer_is_used_when_there_is_no_proxy_header() {
        let info = extract_client_info(&headers(&[]), Some("192.0.2.5:1234".parse().unwrap()));
        assert_eq!(info.ip, "192.0.2.5");
    }

    #[test]
    fn the_forwarded_header_wins_over_the_direct_peer() {
        let info = extract_client_info(
            &headers(&[("x-forwarded-for", "203.0.113.7")]),
            Some("10.0.0.9:1234".parse().unwrap()),
        );
        assert_eq!(info.ip, "203.0.113.7");
    }

    #[test]
    fn an_empty_forwarded_header_falls_through_to_the_peer() {
        let info = extract_client_info(
            &headers(&[("x-forwarded-for", "  ")]),
            Some("192.0.2.5:1234".parse().unwrap()),
        );
        assert_eq!(info.ip, "192.0.2.5");
    }

    #[test]
    fn a_missing_ip_is_recorded_as_empty_rather_than_failing() {
        // Happens for every in-process request and whenever the router is
        // mounted without connect-info. The log row must still be written.
        let info = extract_client_info(&headers(&[]), None);
        assert_eq!(info.ip, "");
    }

    #[test]
    fn the_user_agent_is_captured_verbatim() {
        let info = extract_client_info(&headers(&[("user-agent", "claude-cli/1.2.3")]), None);
        assert_eq!(info.user_agent, "claude-cli/1.2.3");
    }

    #[test]
    fn a_missing_user_agent_is_recorded_as_empty() {
        assert_eq!(extract_client_info(&headers(&[]), None).user_agent, "");
    }

    // ---------- parse_usage ----------

    #[test]
    fn usage_is_read_from_a_buffered_openai_response() {
        let u = parse_usage(br#"{"usage":{"prompt_tokens":5,"completion_tokens":2}}"#).unwrap();
        assert_eq!((u.prompt, u.completion), (5, 2));
    }

    #[test]
    fn usage_is_read_from_a_buffered_anthropic_response() {
        let u = parse_usage(br#"{"usage":{"input_tokens":5,"output_tokens":2}}"#).unwrap();
        assert_eq!((u.prompt, u.completion), (5, 2));
    }

    #[test]
    fn a_response_without_usage_yields_none() {
        assert!(parse_usage(br#"{"choices":[]}"#).is_none());
    }

    #[test]
    fn a_non_json_body_yields_none_rather_than_panicking() {
        assert!(parse_usage(b"<html>gateway timeout</html>").is_none());
    }

    #[test]
    fn a_zeroed_usage_block_yields_none() {
        assert!(parse_usage(br#"{"usage":{"prompt_tokens":0,"completion_tokens":0}}"#).is_none());
    }

    // ---------- push (attempt ordering) ----------

    #[test]
    fn attempts_are_kept_in_call_order() {
        // The log detail page renders the relay chain in the order it happened,
        // so `push` has to append rather than replace.
        let mut v = Vec::new();
        for i in 0..4 {
            v = push(
                v,
                Attempt {
                    upstream_model: format!("m{i}"),
                    channel_name: format!("c{i}"),
                    status: i,
                    error: String::new(),
                    latency_ms: 0,
                    convert: ConvertMode::None,
                    ok: false,
                    skipped: false,
                    usage: None,
                },
            );
        }
        let models: Vec<&str> = v.iter().map(|a| a.upstream_model.as_str()).collect();
        assert_eq!(models, vec!["m0", "m1", "m2", "m3"]);
    }

    // ---------- body_matches_protocol ----------

    #[test]
    fn an_anthropic_message_is_recognized() {
        let body = br#"{"type":"message","id":"m1","content":[]}"#;
        assert!(body_matches_protocol(body, "anthropic"));
    }

    #[test]
    fn an_anthropic_body_without_type_message_is_rejected() {
        let body = br#"{"id":"m1","content":[]}"#;
        assert!(!body_matches_protocol(body, "anthropic"));
    }

    #[test]
    fn an_openai_completion_with_a_message_is_recognized() {
        let body = br#"{"choices":[{"message":{"content":"hi"}}]}"#;
        assert!(body_matches_protocol(body, "openai"));
    }

    #[test]
    fn an_openai_completion_without_a_message_is_rejected() {
        // Relay-station failure mode: 200 + `{"choices":[{"delta":...}]}`
        // (streaming-shaped but never a real completion).
        let body = br#"{"choices":[{"delta":{"content":"x"}}]}"#;
        assert!(!body_matches_protocol(body, "openai"));
    }

    #[test]
    fn non_json_is_rejected_for_both_protocols() {
        let html = b"<html>oops</html>";
        assert!(!body_matches_protocol(html, "anthropic"));
        assert!(!body_matches_protocol(html, "openai"));
    }

    // ---------- should_capture ----------

    /// The point of the new capture rule: failures always capture. The
    /// switch is for *successful* requests, not for "did we get a body" — a
    /// failed request is the one we cannot diagnose later.
    #[tokio::test]
    async fn a_failure_always_captures_regardless_of_the_switch() {
        let state = make_state().await;
        // Switch off (default).
        assert!(should_capture(&state, false));
        // Switch on.
        state
            .debug_logging
            .store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(should_capture(&state, false));
    }

    #[tokio::test]
    async fn a_success_only_captures_when_the_switch_is_on() {
        let state = make_state().await;
        assert!(!should_capture(&state, true));
        state
            .debug_logging
            .store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(should_capture(&state, true));
    }

    async fn make_state() -> AppState {
        // Just need an instance with a working AtomicBool; the relay only
        // touches `debug_logging` for `should_capture`, never `pool`.
        let pool = sqlx::SqlitePool::connect_lazy("sqlite::memory:").expect("lazy connect");
        AppState::new(
            pool,
            Arc::new(crate::breaker::Breaker::new(
                crate::breaker::BreakerConfig::default(),
            )),
        )
    }

    // ---------- DebugCapture ----------

    #[test]
    fn debug_capture_caps_at_the_byte_limit_and_marks_truncation() {
        let cap = DebugCapture::new();
        // Two chunks past the cap. Only the first DEBUG_BODY_MAX bytes stick,
        // and the truncation flag is set as soon as the cap is crossed.
        cap.push(&vec![b'a'; DEBUG_BODY_MAX]);
        cap.push(&[b'b'; 16]);
        let (bytes, seen, truncated) = cap.snapshot();
        assert_eq!(bytes.len(), DEBUG_BODY_MAX);
        assert!(bytes.iter().all(|&b| b == b'a'));
        assert_eq!(seen, DEBUG_BODY_MAX + 16);
        assert!(truncated);
    }

    #[test]
    fn debug_capture_under_the_cap_is_not_marked_truncated() {
        let cap = DebugCapture::new();
        cap.push(b"hello");
        let (bytes, seen, truncated) = cap.snapshot();
        assert_eq!(bytes, b"hello");
        assert_eq!(seen, 5);
        assert!(!truncated);
    }

    // ---------- is_json_content_type ----------

    #[test]
    fn content_type_recognises_json_and_ignores_charset() {
        assert!(is_json_content_type("application/json"));
        assert!(is_json_content_type("application/json; charset=utf-8"));
        assert!(is_json_content_type("application/problem+json"));
        assert!(!is_json_content_type("text/event-stream"));
        assert!(!is_json_content_type("text/plain"));
    }

    // ---------- sse_line_error ----------

    #[test]
    fn an_anthropic_error_event_is_recognised_with_its_own_message() {
        let reason = sse_line_error("event: error").expect("event: error must be caught");
        assert_eq!(reason, STREAM_ERROR_DETAIL);
        let with_payload = sse_line_error(
            "data: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"provider_unavailable\"}}",
        )
        .expect("the error payload must be caught");
        assert_eq!(with_payload, "provider_unavailable");
    }

    #[test]
    fn an_openai_style_in_stream_error_is_recognised_without_an_event_line() {
        // Relays that speak the OpenAI dialect send a bare `data:` error with
        // no `event:` line at all; only watching for `event: error` would miss
        // every one of them.
        let reason = sse_line_error(
            "data: {\"error\":{\"message\":\"upstream node is down\",\"code\":\"internal\"}}",
        )
        .expect("a data-only error envelope must be caught");
        assert_eq!(reason, "upstream node is down");
    }

    #[test]
    fn ordinary_stream_events_are_not_mistaken_for_errors() {
        for line in [
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"gen-1\"}}",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"error: undefined variable\"}}",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}",
            "event: message_stop",
            "data: {\"id\":\"c\",\"choices\":[{\"delta\":{\"content\":\"an error occurred\"}}]}",
            "data: [DONE]",
            ": keep-alive comment",
            "",
        ] {
            assert!(
                sse_line_error(line).is_none(),
                "false positive on {line:?}"
            );
        }
    }

    #[test]
    fn a_crlf_stream_and_a_bare_error_object_both_still_detect() {
        assert!(sse_line_error("event: error\r").is_some());
        // A relay that ships the message key but no type is still an error.
        assert_eq!(
            sse_line_error("data: {\"error\":\"overloaded\"}"),
            Some(STREAM_ERROR_DETAIL.to_string()),
            "a non-object error value is still an error envelope"
        );
    }

    // ---------- sse_frame_end ----------

    #[test]
    fn frame_end_locates_the_first_complete_frame_in_lf_and_crlf_streams() {
        assert_eq!(
            sse_frame_end(b"event: error\ndata: x\n\nrest"),
            Some(b"event: error\ndata: x\n\n".len())
        );
        assert_eq!(
            sse_frame_end(b"event: error\r\ndata: x\r\n\r\nrest"),
            Some(b"event: error\r\ndata: x\r\n\r\n".len())
        );
        assert_eq!(sse_frame_end(b"event: error\ndata: half"), None);
        assert_eq!(sse_frame_end(b""), None);
    }

    #[test]
    fn subsequence_search_handles_empty_needles_and_short_haystacks() {
        assert_eq!(find_subsequence(b"abc", b""), Some(0));
        assert_eq!(find_subsequence(b"", b"x"), None);
        assert_eq!(find_subsequence(b"ab", b"abc"), None);
        assert_eq!(find_subsequence(b"abcabc", b"bc"), Some(1));
    }
}
