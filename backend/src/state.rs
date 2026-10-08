use crate::breaker::Breaker;
use reqwest::Client;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

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
}

impl AppState {
    pub fn new(pool: SqlitePool, breaker: Arc<Breaker>) -> Self {
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
        let global = self
            .proxy_on
            .load(std::sync::atomic::Ordering::Relaxed);
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
        let global = self
            .proxy_on
            .load(std::sync::atomic::Ordering::Relaxed);
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
        self.proxy_on
            .load(std::sync::atomic::Ordering::Relaxed)
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
