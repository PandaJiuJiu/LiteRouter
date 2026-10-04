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
            sessions: Mutex::new(HashMap::new()),
            debug_logging: std::sync::atomic::AtomicBool::new(false),
            breaker,
        }
    }

    /// The client for an outbound call to a channel: the proxied one when the
    /// channel opted in **and** a proxy is actually configured, otherwise the
    /// direct one.
    ///
    /// Falling back to direct rather than erroring is deliberate. A channel
    /// with `use_proxy = 1` and no proxy configured is an admin oversight, and
    /// silently going direct at least keeps the request working in a network
    /// that doesn't need a proxy — hard-failing would instead take the channel
    /// out of service until someone visits the settings page.
    pub fn client_for_channel(&self, use_proxy: bool) -> Client {
        if use_proxy {
            if let Ok(guard) = self.http_proxied.read() {
                if let Some(c) = guard.as_ref() {
                    return c.clone();
                }
            }
        }
        self.http.clone()
    }

    /// Whether a proxied client is currently configured. The UI reads this to
    /// tell the admin that `use_proxy` on a channel is currently a no-op.
    pub fn has_proxied_client(&self) -> bool {
        self.http_proxied
            .read()
            .map(|g| g.is_some())
            .unwrap_or(false)
    }

    /// Install (or clear) the proxied client. Called on startup from
    /// `build_state` and by the admin proxy-settings endpoint, so a config
    /// change takes effect without a restart.
    ///
    /// `proxy_url` is the full URL, scheme included — `reqwest` needs to know
    /// whether it's talking HTTP CONNECT or SOCKS5, and a bare `host:port`
    /// would be parsed as an unknown scheme. `None` clears the client.
    pub fn set_proxied_client(&self, proxy_url: Option<&str>) {
        let mut guard = self
            .http_proxied
            .write()
            .unwrap_or_else(|e| e.into_inner());
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