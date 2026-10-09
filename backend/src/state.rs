use crate::breaker::Breaker;
use reqwest::Client;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::broadcast;

/// Event emitted for a client request's lifecycle on the log page.
///
/// Emitted **twice** per request:
/// - `pending == true`: the request has just arrived and is being relayed.
///   A `logs` row with `status_code = 0` is already committed at this point
///   (`id` is its real primary key), so the in-progress request survives a
///   page refresh. If the pending row insert fails, `id` falls back to `0`.
/// - `pending == false`: the request has completed (success, failure, or
///   stream end) and the same `logs` row has been updated in place. `id` is
///   unchanged — it was assigned when the pending row was written.
///
/// Both events share the same `request_id`, which the frontend uses to match
/// the "in progress" row to its final result.
#[derive(Clone, serde::Serialize)]
pub struct LogEvent {
    /// Real `logs.id` — assigned when the pending row is inserted, and kept
    /// when the row is updated on completion. `0` only if the pending insert
    /// failed and the row is written later.
    pub id: i64,
    /// Per-request correlation key, identical across the pending and final
    /// events for one client request. Format: `{token}|{model}|{nanos}`.
    pub request_id: String,
    pub token_name: String,
    pub request_model: String,
    pub upstream_model: String,
    pub channel_name: String,
    pub status_code: i64,
    pub latency_ms: i64,
    pub total_tokens: i64,
    pub created_at: i64,
    pub failed_count: i64,
    pub client_ip: String,
    pub user_agent: String,
    pub client_aborted: bool,
    pub streaming: bool,
    pub protocol: String,
    /// `true` = request in flight (a `status_code = 0` row in `logs`);
    /// `false` = request settled and the same row has been updated.
    pub pending: bool,
}

/// One issued session. Carries enough info to authorize requests without
/// hitting the DB on every check. In-memory only: restarts clear sessions,
/// which matches the existing behavior.
#[derive(Clone)]
pub struct SessionInfo {
    pub user_id: i64,
    pub is_admin: bool,
}

/// Shared 600s budget for every outbound call — a relay request is allowed to
/// take minutes (large prompts, long streams, reasoning models), so the
/// timeout is a safety net against a hung upstream, not a latency budget.
const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

pub struct AppState {
    pub pool: SqlitePool,
    /// Direct client (no proxy) — always present.
    pub http: Client,
    /// Client routed through the configured proxy, or `None` while no proxy
    /// is configured. Behind an `RwLock` rather than a plain field because the
    /// admin settings endpoint swaps it while requests are in flight: a
    /// `Proxy` baked into a `Client` is immutable, so a change means building
    /// a whole new client and every later request has to pick it up. Reads are
    /// a lock + clone, and `reqwest::Client` clones are an `Arc` bump — this
    /// is not on any hot path that cares.
    http_proxied: RwLock<Option<Client>>,
    /// Whether the configured proxy should actually be used. Read on every
    /// outbound call via [`Self::client_for_channel`], so flipping it from
    /// the settings handler takes effect without rebuilding the cached client
    /// (and without touching the host/port config). Independent of host/port:
    /// configuring a proxy server does NOT implicitly enable it — the admin
    /// has to flip this on, otherwise the configured host/port is dormant.
    /// Atomic because the admin handler mutates it without a lock and
    /// requests are reading it concurrently.
    proxy_on: std::sync::atomic::AtomicBool,
    /// session token -> metadata
    pub sessions: Mutex<HashMap<String, SessionInfo>>,
    /// Whether request/response bodies are captured to disk. Held on the
    /// state itself (not a process-wide static) so each harness can flip
    /// it independently — important for parallel tests. Updated atomically
    /// since the admin handler mutates it without a lock.
    pub debug_logging: std::sync::atomic::AtomicBool,
    /// Per-(channel, model) circuit breaker. In-memory only — process
    /// restart clears every key, matching the session-stores-don't-
    /// survive-restart stance. See `breaker.rs` for the state machine.
    pub breaker: Arc<Breaker>,
    /// Broadcast channel for SSE log stream. Large capacity to handle burst
    /// of requests. Dropping the sender stops the stream.
    /// Capacity 1000: each request emits two events (pending + final), so the
    /// effective per-request headroom is 500 concurrent in-flight requests
    /// without losing events.
    pub log_sender: broadcast::Sender<LogEvent>,
}

impl AppState {
    pub fn new(pool: SqlitePool, breaker: Arc<Breaker>) -> Self {
        let log_sender = broadcast::channel::<LogEvent>(1000).0;
        Self {
            pool,
            http: Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .expect("build http client"),
            http_proxied: RwLock::new(None),
            proxy_on: std::sync::atomic::AtomicBool::new(false),
            sessions: Mutex::new(HashMap::new()),
            debug_logging: std::sync::atomic::AtomicBool::new(false),
            breaker,
            log_sender,
        }
    }

    /// The client for an outbound call to a channel: the proxied one when
    /// the call will actually go through the proxy.
    ///
    /// Two orthogonal reasons the call goes through the proxy:
    /// - The channel itself opted in via `use_proxy` (per-channel toggle).
    /// - The global switch is on, in which case every channel goes through
    ///   the proxy regardless of its own setting.
    ///
    /// These combine as **OR**: any one of them being on is enough to route
    /// this call through the proxy. Per-channel and global are independent
    /// settings — a channel with `use_proxy = 1` works fine with the global
    /// switch off, and a channel with `use_proxy = 0` is still proxied when
    /// the global switch is on. (Earlier the per-channel toggle only took
    /// effect when global was on; that coupled two unrelated decisions and
    /// is what this method no longer does.)
    ///
    /// On top of either of those, the proxy server itself has to be
    /// configured (host + port). Falling back to direct rather than erroring
    /// is deliberate — a misconfiguration should not take a channel out of
    /// service.
    pub fn client_for_channel(&self, use_proxy: bool) -> Client {
        let global = self.proxy_on.load(std::sync::atomic::Ordering::Relaxed);
        if use_proxy || global {
            if let Ok(guard) = self.http_proxied.read() {
                if let Some(c) = guard.as_ref() {
                    return c.clone();
                }
            }
        }
        self.http.clone()
    }

    /// Whether this specific channel will actually be routed through the
    /// proxy at request time. Combines (a) the global switch, (b) the
    /// channel's own `use_proxy`, and (c) whether the proxy server itself is
    /// configured — any one of (a)/(b) being on plus (c) is enough. The UI
    /// uses this to render the per-channel "代理" tag so it doesn't claim a
    /// proxy is in use when it isn't.
    pub fn proxy_active_for(&self, channel_use_proxy: bool) -> bool {
        let global = self.proxy_on.load(std::sync::atomic::Ordering::Relaxed);
        if !channel_use_proxy && !global {
            return false;
        }
        self.http_proxied
            .read()
            .map(|g| g.is_some())
            .unwrap_or(false)
    }

    /// Whether the global proxy switch is on. Read on every outbound call
    /// via [`Self::client_for_channel`], so flipping it from the settings
    /// handler takes effect without rebuilding the cached client.
    /// Independent of host/port: configuring a proxy server does NOT
    /// implicitly enable it — the admin has to flip this on.
    /// Atomic because the admin handler mutates it without a lock and
    /// requests are reading it concurrently.
    pub fn proxy_on(&self) -> bool {
        self.proxy_on.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Flip the global proxy switch.
    pub fn set_proxy_on(&self, enabled: bool) {
        self.proxy_on
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    /// Install (or clear) the proxied client. Called on startup from
    /// `build_state` and by the admin proxy-settings endpoint when
    /// host/port change, so a config change takes effect without a
    /// restart.
    ///
    /// `proxy_url` is the full URL, scheme included — `reqwest` needs to know
    /// whether it's talking HTTP CONNECT or SOCKS5, and a bare `host:port`
    /// would be parsed as an unknown scheme. `None` clears the client.
    /// Note: this only sets/clears the cached client; whether it actually
    /// gets used is gated by [`Self::set_proxy_on`].
    pub fn set_proxied_client(&self, proxy_url: Option<&str>) {
        let mut guard = self.http_proxied.write().unwrap_or_else(|e| e.into_inner());
        match proxy_url {
            None => *guard = None,
            Some(url) => match reqwest::Proxy::all(url) {
                Ok(proxy) => match Client::builder().timeout(HTTP_TIMEOUT).proxy(proxy).build() {
                    Ok(c) => *guard = Some(c),
                    Err(e) => {
                        // Keep serving direct rather than disabling the
                        // gateway; the settings endpoint validates the URL
                        // before it ever gets here, so this is near-unreachable.
                        eprintln!("build proxied http client failed: {e}");
                        *guard = None;
                    }
                },
                Err(e) => {
                    eprintln!("invalid proxy url: {e}");
                    *guard = None;
                }
            },
        }
    }
}
