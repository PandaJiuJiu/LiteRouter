use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

pub async fn init_pool(path: &str) -> SqlitePool {
    let url = format!("sqlite:{}?mode=rwc", path);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("failed to open sqlite database");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS channels (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            name        TEXT NOT NULL,
            base_url    TEXT NOT NULL DEFAULT '',
            base_url_anthropic TEXT NOT NULL DEFAULT '',
            api_key     TEXT NOT NULL,
            models      TEXT NOT NULL DEFAULT '',
            enabled     INTEGER NOT NULL DEFAULT 1,
            kind        TEXT NOT NULL DEFAULT 'external',
            created_at  INTEGER NOT NULL
        );",
    )
    .execute(&pool)
    .await
    .expect("create channels table");
    // migrate existing databases (add column if missing)
    let _ = sqlx::query(
        "ALTER TABLE channels ADD COLUMN base_url_anthropic TEXT NOT NULL DEFAULT ''",
    )
    .execute(&pool)
    .await;
    // channel kind: 'external' channels serve relay traffic (all incoming
    // requests are internal); 'internal' channels are excluded from routing
    let _ = sqlx::query(
        "ALTER TABLE channels ADD COLUMN kind TEXT NOT NULL DEFAULT 'external'",
    )
    .execute(&pool)
    .await;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS tokens (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            name        TEXT NOT NULL,
            key         TEXT NOT NULL UNIQUE,
            enabled     INTEGER NOT NULL DEFAULT 1,
            created_at  INTEGER NOT NULL,
            accessed_at INTEGER NOT NULL DEFAULT 0,
            rpm_limit         INTEGER NOT NULL DEFAULT 0,
            daily_token_limit INTEGER NOT NULL DEFAULT 0
        );",
    )
    .execute(&pool)
    .await
    .expect("create tokens table");
    // migrate: add quota columns if missing (idempotent)
    for col in ["rpm_limit", "daily_token_limit"] {
        let _ = sqlx::query(&format!(
            "ALTER TABLE tokens ADD COLUMN {} INTEGER NOT NULL DEFAULT 0",
            col
        ))
        .execute(&pool)
        .await;
    }
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS logs (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            token_name   TEXT NOT NULL,
            model        TEXT NOT NULL,
            channel_name TEXT NOT NULL,
            status_code  INTEGER NOT NULL,
            prompt_tokens     INTEGER NOT NULL DEFAULT 0,
            completion_tokens INTEGER NOT NULL DEFAULT 0,
            total_tokens      INTEGER NOT NULL DEFAULT 0,
            created_at   INTEGER NOT NULL
        );",
    )
    .execute(&pool)
    .await
    .expect("create logs table");
    // migrate existing databases: add token columns if missing (idempotent)
    for col in ["prompt_tokens", "completion_tokens", "total_tokens"] {
        let _ = sqlx::query(&format!(
            "ALTER TABLE logs ADD COLUMN {} INTEGER NOT NULL DEFAULT 0",
            col
        ))
        .execute(&pool)
        .await;
    }
    // index for per-token usage lookups (RPM check + daily quota check)
    let _ = sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_logs_token_created ON logs (token_name, created_at)",
    )
    .execute(&pool)
    .await;
    // model aliases: rewrite a user-supplied name (e.g. "gpt-4o") into the
    // upstream's real model id before channel routing
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS model_mappings (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            alias        TEXT NOT NULL UNIQUE,
            target_model TEXT NOT NULL,
            created_at   INTEGER NOT NULL
        );",
    )
    .execute(&pool)
    .await
    .expect("create model_mappings table");
    // one alias may route to several upstream models, tried in array order;
    // stored as a JSON array of model-id strings
    let _ = sqlx::query(
        "ALTER TABLE model_mappings ADD COLUMN targets TEXT NOT NULL DEFAULT ''",
    )
    .execute(&pool)
    .await;
    // migrate: fold pre-existing single-target rows into the new JSON column
    sqlx::query(
        "UPDATE model_mappings SET targets = json_array(target_model) WHERE targets = '' OR targets IS NULL",
    )
    .execute(&pool)
    .await
    .expect("migrate model_mappings targets");
    pool
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
