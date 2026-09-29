use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct AppState {
    pub pool: SqlitePool,
    pub http: reqwest::Client,
    /// admin session token -> created_at (in-memory, restart clears sessions)
    pub sessions: Mutex<HashMap<String, i64>>,
}

impl AppState {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .expect("build http client"),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn admin_password() -> String {
        std::env::var("ADMIN_PASSWORD").unwrap_or_else(|_| "admin123".to_string())
    }
}
