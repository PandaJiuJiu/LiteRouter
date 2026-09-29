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
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS tokens (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            name        TEXT NOT NULL,
            key         TEXT NOT NULL UNIQUE,
            enabled     INTEGER NOT NULL DEFAULT 1,
            created_at  INTEGER NOT NULL,
            accessed_at INTEGER NOT NULL DEFAULT 0
        );",
    )
    .execute(&pool)
    .await
    .expect("create tokens table");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS logs (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            token_name   TEXT NOT NULL,
            model        TEXT NOT NULL,
            channel_name TEXT NOT NULL,
            status_code  INTEGER NOT NULL,
            created_at   INTEGER NOT NULL
        );",
    )
    .execute(&pool)
    .await
    .expect("create logs table");
    pool
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
