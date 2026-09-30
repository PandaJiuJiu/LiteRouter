use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use sha2::Sha256;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

/// PBKDF2-HMAC-SHA256, 100k iterations (OWASP 2023 minimum), 32-byte output.
const PBKDF2_ITERATIONS: u32 = 100_000;
const PBKDF2_OUTPUT_LEN: usize = 32;
const SALT_LEN: usize = 16;

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
    // migrate: drop the obsolete `kind` column. The 'internal' vs 'external'
    // distinction was redundant with `enabled` and confusingly named, so we
    // removed it. SQLite ≥3.35 supports DROP COLUMN.
    let _ = sqlx::query("ALTER TABLE channels DROP COLUMN kind")
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
    // migrate: add per-user ownership for multi-account isolation. NULL =
    // legacy / orphaned token, visible only to admins.
    let _ = sqlx::query("ALTER TABLE tokens ADD COLUMN user_id INTEGER")
        .execute(&pool)
        .await;
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
            created_at   INTEGER NOT NULL,
            request_model TEXT NOT NULL DEFAULT ''
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
    // migrate: track the original client-requested model alongside the
    // upstream model the request was actually rewritten to (model column).
    let _ = sqlx::query(
        "ALTER TABLE logs ADD COLUMN request_model TEXT NOT NULL DEFAULT ''",
    )
    .execute(&pool)
    .await;
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

    // ---------- multi-account: users table + one-time migrations ----------
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            username      TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            password_salt TEXT NOT NULL,
            is_admin      INTEGER NOT NULL DEFAULT 0,
            created_at    INTEGER NOT NULL,
            updated_at    INTEGER NOT NULL
        );",
    )
    .execute(&pool)
    .await
    .expect("create users table");

    // First-boot bootstrap: if no users exist, either seed from the legacy
    // ADMIN_PASSWORD env var (existing deployments) or leave the table empty
    // and let the web setup wizard create the first admin (new deployments).
    let user_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap_or(0);
    if user_count == 0 {
        if let Ok(env_pw) = std::env::var("ADMIN_PASSWORD") {
            if !env_pw.trim().is_empty() {
                let (hash, salt) = hash_password(&env_pw);
                let ts = now();
                sqlx::query(
                    "INSERT INTO users (username, password_hash, password_salt, is_admin, created_at, updated_at)
                     VALUES (?, ?, ?, 1, ?, ?)",
                )
                .bind("admin")
                .bind(&hash)
                .bind(&salt)
                .bind(ts)
                .bind(ts)
                .execute(&pool)
                .await
                .expect("bootstrap admin from env");
                eprintln!(
                    "literouter: bootstrapped default admin from ADMIN_PASSWORD env (username: admin)"
                );
            } else {
                eprintln!(
                    "literouter: no users exist; visit the web UI to run the setup wizard"
                );
            }
        } else {
            eprintln!(
                "literouter: no users exist; visit the web UI to run the setup wizard"
            );
        }
    }

    // Adopt orphaned (legacy) tokens into the first admin's account so they
    // remain manageable. Only runs after at least one user exists.
    let orphan_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tokens WHERE user_id IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or(0);
    if orphan_count > 0 {
        let first_admin: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE is_admin=1 ORDER BY id ASC LIMIT 1",
        )
        .fetch_optional(&pool)
        .await
        .unwrap_or(None);
        if let Some(admin_id) = first_admin {
            sqlx::query("UPDATE tokens SET user_id = ? WHERE user_id IS NULL")
                .bind(admin_id)
                .execute(&pool)
                .await
                .expect("adopt orphan tokens");
        }
    }

    pool
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Hash a plaintext password. Returns (hex_hash, hex_salt). Caller persists
/// both columns; verification needs them together.
pub fn hash_password(plain: &str) -> (String, String) {
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut out = [0u8; PBKDF2_OUTPUT_LEN];
    pbkdf2_hmac::<Sha256>(plain.as_bytes(), &salt, PBKDF2_ITERATIONS, &mut out);
    (hex::encode(out), hex::encode(salt))
}

/// Constant-time verification. Computes the hash with the same salt+rounds,
/// then compares in constant time. Returns false on any decode error rather
/// than panicking — wrong-format hashes are an authentication failure, not
/// a server bug.
pub fn verify_password(plain: &str, hash_hex: &str, salt_hex: &str) -> bool {
    let salt = match hex::decode(salt_hex) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let expected = match hex::decode(hash_hex) {
        Ok(h) => h,
        Err(_) => return false,
    };
    let mut out = vec![0u8; expected.len()];
    pbkdf2::pbkdf2_hmac::<Sha256>(plain.as_bytes(), &salt, PBKDF2_ITERATIONS, &mut out);
    if out.len() != expected.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in out.iter().zip(expected.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Delete log rows older than `retention_days` days. Returns the number of
/// rows removed so the caller can log it. Called by the background sweeper
/// in main.rs so the `logs` table doesn't grow unbounded under relay load.
pub async fn cleanup_old_logs(
    pool: &SqlitePool,
    retention_days: i64,
) -> Result<u64, sqlx::Error> {
    let cutoff = now() - retention_days * 86400;
    let res = sqlx::query("DELETE FROM logs WHERE created_at < ?")
        .bind(cutoff)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}