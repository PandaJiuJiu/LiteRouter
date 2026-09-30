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
use std::sync::OnceLock;

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
    let id = sqlx::query("INSERT INTO tokens (name, key, enabled, created_at, user_id) VALUES (?,?,1,?,?)")
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
    ChannelRow { id: id, name: name.to_string() }
}

/// Route `alias` to a single `{channel: "", model}` target — i.e. "any
/// channel that serves this model", which is what failover walks.
pub async fn insert_mapping_any(pool: &SqlitePool, alias: &str, model: &str) -> i64 {
    insert_mapping_targets(pool, alias, &json!([{ "channel": "", "model": model }])).await
}

/// Route `alias` to an explicit ordered target list.
pub async fn insert_mapping_targets(pool: &SqlitePool, alias: &str, targets: &Value) -> i64 {
    sqlx::query("INSERT INTO model_mappings (alias, target_model, targets, created_at) VALUES (?,?,?,?)")
        .bind(alias)
        .bind("")
        .bind(targets.to_string())
        .bind(db::now())
        .execute(pool)
        .await
        .expect("insert mapping")
        .last_insert_rowid()
}

/// Mount the full API over a database, exactly as `main` would.
pub async fn mount(pool: SqlitePool) -> Router {
    let state = build_state(pool).await;
    build_router(state)
}

/// `TestDb` + mounted `Router`, the pair almost every API test needs.
pub struct Harness {
    pub db: TestDb,
    pub router: Router,
}

impl Harness {
    pub async fn new() -> Self {
        let db = TestDb::new().await;
        let router = mount(db.pool.clone()).await;
        Self { db, router }
    }

    /// A harness that already has an `admin` user.
    pub async fn with_admin() -> Self {
        let db = TestDb::with_admin().await;
        let router = mount(db.pool.clone()).await;
        Self { db, router }
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
    let mut req = Request::builder().method(method).uri(uri).header("content-type", "application/json");
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

use tower::ServiceExt;
