use crate::breaker::{Breaker, BreakerConfig};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// One issued session. Carries enough info to authorize requests without
/// hitting the DB on every check. In-memory only: restarts clear sessions,
/// which matches the existing behavior. `created_at` is unused today but
/// reserved so a future session-expiry sweep has the field ready.
#[derive(Clone)]
pub struct SessionInfo {
    pub user_id: i64,
    pub is_admin: bool,
    #[allow(dead_code)]
    pub created_at: i64,
}

pub struct AppState {
    pub pool: SqlitePool,
    pub http: reqwest::Client,
    /// session token -> metadata
    pub sessions: Mutex<HashMap<String, SessionInfo>>,
    /// When true, request/response bodies are written to data/debug_logs/.
    pub debug_logging: bool,
    /// Per-(channel, model) circuit breaker. In-memory only — process
    /// restart clears every key, matching the session-stores-don't-
    /// survive-restart stance. See `breaker.rs` for the state machine.
    pub breaker: Arc<Breaker>,
}

impl AppState {
    pub fn new(pool: SqlitePool, debug_logging: bool, breaker: Arc<Breaker>) -> Self {
        Self {
            pool,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .expect("build http client"),
            sessions: Mutex::new(HashMap::new()),
            debug_logging,
            breaker,
        }
    }

    /// Convenience for tests / hot paths that don't have a Breaker handy.
    /// Production startup always uses `new(...)`.
    #[allow(dead_code)]
    pub fn with_default_breaker(pool: SqlitePool, debug_logging: bool) -> Self {
        let breaker = Arc::new(Breaker::new(BreakerConfig::default()));
        Self::new(pool, debug_logging, breaker)
    }
}
