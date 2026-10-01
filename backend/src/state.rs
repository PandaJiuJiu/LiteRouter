use crate::breaker::Breaker;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// One issued session. Carries enough info to authorize requests without
/// hitting the DB on every check. In-memory only: restarts clear sessions,
/// which matches the existing behavior.
#[derive(Clone)]
pub struct SessionInfo {
    pub user_id: i64,
    pub is_admin: bool,
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
}
