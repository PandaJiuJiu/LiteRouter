//! Shared fixtures for the integration test suites.
//!
//! Every test gets its **own SQLite file** under a tempdir. That is not
//! paranoia: SQLite serializes writers per-database, so a shared in-memory
//! pool would make parallel `cargo test` runs intermittently hit
//! `database is locked`. A file per test costs one `migrate!()` pass each and
//! removes the whole class of flake.
//!
//! Password hashing is the other cost that would otherwise dominate the run:
//! `db::hash_password` runs 100k PBKDF2 rounds by design. Rather than weaken
//! the production work factor (or add an env var that could weaken it in
//! production too), every fixture shares **one** precomputed hash — computed
//! lazily once per test binary. Same password, same salt, so correctness is
//! unaffected and the KDF runs once instead of once per seeded user.

#![allow(dead_code)] // each test binary uses a different subset of these

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::Router;
use literouter::{build_router, build_state, db};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::sync::{Arc, OnceLock};

/// The single password every fixture user is created with.
pub const PASSWORD: &str = "test-password-1234";

/// PBKDF2 output for [`PASSWORD`], computed at most once per test binary.
fn shared_hash() -> &'static (String, String) {
    static H: OnceLock<(String, String)> = OnceLock::new();
    H.get_or_init(|| db::hash_password(PASSWORD))
}

/// A tempdir-backed database. Holds the pool alive; the file is removed on
/// drop.
pub struct TestDb {
    pub pool: SqlitePool,
    _dir: tempfile::TempDir,
}

impl TestDb {
    /// A migrated, empty database — the state a fresh install boots into.
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.db");
        let pool = db::init_pool(&path.to_string_lossy()).await;
        Self { pool, _dir: dir }
    }

    /// A database with an admin user already present.
    pub async fn with_admin() -> Self {
        let db = Self::new().await;
        insert_user(&db.pool, "admin", true).await;
        db
    }
}

pub async fn insert_user(pool: &SqlitePool, username: &str, is_admin: bool) -> i64 {
    let (hash, salt) = shared_hash();
    let ts = db::now();
    sqlx::query(
        "INSERT INTO users (username, password_hash, password_salt, is_admin, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(username)
    .bind(hash)
    .bind(salt)
    .bind(if is_admin { 1 } else { 0 })
    .bind(ts)
    .bind(ts)
    .execute(pool)
    .await
    .expect("insert user")
    .last_insert_rowid()
}

pub struct TokenRow {
    pub id: i64,
    pub key: String,
    pub user_id: i64,
}

/// Create an `sk-` token owned by `user_id`.
pub async fn insert_token(pool: &SqlitePool, name: &str, user_id: i64) -> TokenRow {
    let key = format!("sk-test-{name}-{}", db::now());
    let id = sqlx::query(
        "INSERT INTO tokens (name, key, enabled, created_at, user_id) VALUES (?,?,1,?,?)",
    )
    .bind(name)
    .bind(&key)
    .bind(db::now())
    .bind(user_id)
    .execute(pool)
    .await
    .expect("insert token")
    .last_insert_rowid();
    TokenRow { id, key, user_id }
}

pub struct ChannelRow {
    pub id: i64,
    pub name: String,
}

/// Create a channel serving `models` (comma separated) on the given base URLs.
/// Empty URL strings mean "this protocol is not offered".
pub async fn insert_channel(
    pool: &SqlitePool,
    name: &str,
    base_url: &str,
    base_url_anthropic: &str,
    models: &str,
    enabled: bool,
) -> ChannelRow {
    let id = sqlx::query(
        "INSERT INTO channels (name, base_url, base_url_anthropic, api_key, models, enabled, created_at)
         VALUES (?,?,?,?,?,?,?)",
    )
    .bind(name)
    .bind(base_url)
    .bind(base_url_anthropic)
    .bind("sk-upstream-secret")
    .bind(models)
    .bind(if enabled { 1 } else { 0 })
    .bind(db::now())
    .execute(pool)
    .await
    .expect("insert channel")
    .last_insert_rowid();
    ChannelRow {
        id,
        name: name.to_string(),
    }
}

/// Route `alias` to a single `{channel: "", model}` target — i.e. "any
/// channel that serves this model", which is what failover walks.
pub async fn insert_mapping_any(pool: &SqlitePool, alias: &str, model: &str) -> i64 {
    insert_mapping_targets(pool, alias, &json!([{ "channel": "", "model": model }])).await
}

/// Route `alias` to an explicit ordered target list.
pub async fn insert_mapping_targets(pool: &SqlitePool, alias: &str, targets: &Value) -> i64 {
    sqlx::query(
        "INSERT INTO model_mappings (alias, target_model, targets, created_at) VALUES (?,?,?,?)",
    )
    .bind(alias)
    .bind("")
    .bind(targets.to_string())
    .bind(db::now())
    .execute(pool)
    .await
    .expect("insert mapping")
    .last_insert_rowid()
}

/// The debug-capture directory the relay actually uses for this pool. Tests
/// assert against files written by the relay, so they have to look in the
/// same place — derived from the pool's file path so parallel tests don't
/// collide.
pub fn debug_log_dir(pool: &sqlx::SqlitePool) -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("literouter-debug-logs-{}", std::process::id()))
        .join(
            pool.connect_options()
                .as_ref()
                .clone()
                .get_filename()
                .to_string_lossy()
                .replace('/', "_"),
        )
}

/// Point the relay's debug-capture directory at a temp dir keyed by the test's
/// pool, so parallel tests don't overwrite each other's captures. The relay's
/// capture directory is keyed by the pool's underlying SQLite file path,
/// which `TestDb` makes unique per test — that's how the redirect reaches a
/// stable, race-free path.
pub fn redirect_debug_logs(pool: &sqlx::SqlitePool) {
    let base = std::env::temp_dir()
        .join(format!("literouter-debug-logs-{}", std::process::id()))
        .join(
            pool.connect_options()
                .as_ref()
                .clone()
                .get_filename()
                .to_string_lossy()
                .replace('/', "_"),
        );
    literouter::proxy::set_debug_log_dir_for(pool, base.to_str().unwrap());
}

/// A harness with debug logging pre-enabled for the relays it builds. Use
/// when a test needs to assert against the capture of a *successful* request
/// — failures capture unconditionally and don't need it.
pub async fn harness_with_debug(pool: SqlitePool) -> (Router, Arc<literouter::breaker::Breaker>) {
    redirect_debug_logs(&pool);
    let state = build_state(pool).await;
    state
        .debug_logging
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let breaker = state.breaker.clone();
    (build_router(state), breaker)
}

/// Mount the full API over a database, exactly as `main` would. The
/// breaker comes back too so tests can pre-trip it without first having to
/// drive N real failures through the relay.
pub async fn mount(pool: SqlitePool) -> (Router, Arc<literouter::breaker::Breaker>) {
    redirect_debug_logs(&pool);
    let state = build_state(pool).await;
    let breaker = state.breaker.clone();
    (build_router(state), breaker)
}

/// `TestDb` + mounted `Router`, the pair almost every API test needs.
pub struct Harness {
    pub db: TestDb,
    pub router: Router,
    /// The same `Arc<Breaker>` the mounted router is using.
    pub breaker: Arc<literouter::breaker::Breaker>,
    /// `Some` if the harness was built with debug logging pre-enabled; tests
    /// that need to assert against a successful request's capture use this
    /// to flip the per-state switch without affecting other harnesses.
    pub state: Arc<literouter::state::AppState>,
}

impl Harness {
    pub async fn new() -> Self {
        let db = TestDb::new().await;
        redirect_debug_logs(&db.pool);
        let state = build_state(db.pool.clone()).await;
        let breaker = state.breaker.clone();
        let router = build_router(state.clone());
        Self {
            db,
            router,
            breaker,
            state,
        }
    }

    /// A harness that already has an `admin` user.
    pub async fn with_admin() -> Self {
        let db = TestDb::with_admin().await;
        redirect_debug_logs(&db.pool);
        let state = build_state(db.pool.clone()).await;
        let breaker = state.breaker.clone();
        let router = build_router(state.clone());
        Self {
            db,
            router,
            breaker,
            state,
        }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.db.pool
    }
}

/// Issue one request against the mounted router without binding a port.
pub async fn call(
    router: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> axum::response::Response {
    let mut b = Body::empty();
    if let Some(v) = body {
        b = Body::from(serde_json::to_vec(&v).unwrap());
    }
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    router
        .clone()
        .oneshot(req.body(b).unwrap())
        .await
        .expect("router call")
}

/// Same as [`call`] but with an explicit peer address, so
/// `X-Forwarded-For`-less client-IP logging can be exercised.
pub async fn call_from(
    router: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
    peer: std::net::SocketAddr,
) -> axum::response::Response {
    let mut b = Body::empty();
    if let Some(v) = body {
        b = Body::from(serde_json::to_vec(&v).unwrap());
    }
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let mut req = req.body(b).unwrap();
    req.extensions_mut().insert(ConnectInfo(peer));
    router.clone().oneshot(req).await.expect("router call")
}

/// Same as [`call`] but decodes a JSON response body.
pub async fn call_json(
    router: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let resp = call(router, method, uri, body, bearer).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
        .await
        .expect("read body");
    let v = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, v)
}

/// Log in over the real HTTP surface and return the session id.
pub async fn login(router: &Router, username: &str) -> String {
    let (status, body) = call_json(
        router,
        "POST",
        "/api/login",
        Some(json!({ "username": username, "password": PASSWORD })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login failed: {body}");
    body["session"].as_str().expect("session").to_string()
}

use std::time::Duration;
use tokio::time::{interval, MissedTickBehavior};
use tower::ServiceExt;

/// Poll the DB until at least `expected` settled rows exist in `logs`, or
/// `timeout` elapses. Use this after a relay request when the backend logs
/// asynchronously off the response hot path.
///
/// Rows with `status_code = 0` are "in progress" (written when the request
/// arrived) and are not settled yet — they only count once finalized, so a
/// test that waits for one log row won't accidentally read a pending row.
pub async fn wait_for_logs(pool: &SqlitePool, expected: i64, timeout: Duration) {
    let mut ticker = interval(Duration::from_millis(10));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM logs WHERE status_code <> 0")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
        if count >= expected {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("timed out waiting for {} log row(s)", expected);
        }
        ticker.tick().await;
    }
}

/// Poll the DB until a log row with the given id has debug capture on disk.
pub async fn wait_for_debug(pool: &SqlitePool, log_id: i64, timeout: Duration) {
    let mut ticker = interval(Duration::from_millis(10));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let deadline = tokio::time::Instant::now() + timeout;
    let dir = debug_log_dir(pool);
    loop {
        if dir.join(log_id.to_string()).join("resp.json").exists() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("timed out waiting for debug capture for log {}", log_id);
        }
        ticker.tick().await;
    }
}
