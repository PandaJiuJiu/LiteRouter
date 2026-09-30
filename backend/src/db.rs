use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use sha2::Sha256;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

/// PBKDF2-HMAC-SHA256, 100k iterations (OWASP 2023 minimum), 32-byte output.
const PBKDF2_ITERATIONS: u32 = 100_000;
const PBKDF2_OUTPUT_LEN: usize = 32;
const SALT_LEN: usize = 16;

/// Open the pool and apply any pending schema migrations. All schema lives
/// in `backend/migrations/*.sql` and is owned by sqlx::migrate!; this
/// function should not contain any inline `CREATE TABLE` / `ALTER TABLE`.
pub async fn init_pool(path: &str) -> SqlitePool {
    let url = format!("sqlite:{}?mode=rwc", path);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("failed to open sqlite database");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply schema migrations");

    // Adopt legacy orphan tokens into the first admin so they're manageable
    // in the UI. No-op on a fresh DB (no tokens yet) and on a fully-migrated
    // DB (no NULL user_id left); only matters for the multi-account cut-over.
    let first_admin: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM users WHERE is_admin = 1 ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap_or(None);
    if let Some(admin_id) = first_admin {
        let _ = sqlx::query("UPDATE tokens SET user_id = ? WHERE user_id IS NULL")
            .bind(admin_id)
            .execute(&pool)
            .await;
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