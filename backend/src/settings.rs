use crate::auth::{self, require_admin};
use crate::breaker::BreakerConfig;
use crate::db;
use crate::proxy;
use crate::state::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// GET /api/settings/debug-logging — whether request/response bodies are
/// being written to disk for debugging.
pub async fn get_debug_logging(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    let _user = require_admin(&state, &headers)?;

    let enabled = db::get_setting(&state.pool, "debug_logging")
        .await
        .ok()
        .flatten()
        .map(|v| v == "1")
        .unwrap_or(false);

    Ok(Json(json!({ "enabled": enabled })))
}

#[derive(Deserialize)]
pub struct SetDebugLoggingReq {
    enabled: bool,
}

/// PUT /api/settings/debug-logging — toggle debug body logging. Updates both
/// the DB and the in-memory flag so the change is effective immediately.
pub async fn set_debug_logging(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SetDebugLoggingReq>,
) -> Result<Json<Value>, StatusCode> {
    let _user = require_admin(&state, &headers)?;

    let value = if req.enabled { "1" } else { "0" };
    let _ = sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('debug_logging', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(value)
    .execute(&state.pool)
    .await;

    proxy::set_debug_logging(&state, req.enabled);

    Ok(Json(json!({ "enabled": req.enabled })))
}

// ---------- UI language ----------

/// The only two UI languages the SPA ships. Anything else is rejected on
/// write so a stray value can't end up in the DB and break rendering.
pub const LANGUAGES: [&str; 2] = ["zh-CN", "en-US"];

/// Coerce a client-supplied language into one of [`LANGUAGES`], falling back
/// to Chinese. Used on the setup path where the field is optional and an
/// unknown value should not fail account creation.
pub fn normalize_language(raw: Option<&str>) -> String {
    match raw {
        Some(v) if LANGUAGES.contains(&v) => v.to_string(),
        _ => LANGUAGES[0].to_string(),
    }
}

/// Persist the UI language. UPSERT so it works whether or not the migration
/// has seeded the key.
pub async fn store_language(pool: &sqlx::SqlitePool, lang: &str) {
    let _ = sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('ui_language', ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(lang)
    .execute(pool)
    .await;
}

/// Read the configured UI language. Defaults to Chinese if the row is missing
/// or unreadable — never fail a request over a cosmetic setting.
pub async fn current_language(pool: &sqlx::SqlitePool) -> String {
    normalize_language(
        db::get_setting(pool, "ui_language")
            .await
            .ok()
            .flatten()
            .as_deref(),
    )
}

/// GET /api/settings/language — public. The login and setup pages need to
/// render in the right language before any session exists.
pub async fn get_language(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({ "language": current_language(&state.pool).await }))
}

#[derive(Deserialize)]
pub struct SetLanguageReq {
    language: String,
}

/// PUT /api/settings/language — any signed-in user may switch the UI
/// language; it is a global preference, not an admin-only setting.
pub async fn set_language(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SetLanguageReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let _user = auth::check_auth(&state, &headers)
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(json!({ "error": "未登录" }))))?;

    if !LANGUAGES.contains(&req.language.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": format!("不支持的语言：{}", req.language) }).into(),
        ));
    }

    store_language(&state.pool, &req.language).await;
    Ok(Json(json!({ "language": req.language })))
}

// ---------- log retention ----------
//
// How many days of `logs` rows to keep. The hourly sweep in main.rs reads
// this on every cycle so an admin's edit takes effect on the next sweep,
// not on the next process restart. The dropdown offers four preset windows
// — chosen to match the operational windows an LLM gateway is typically
// asked to support — and the server rejects anything outside `[1, 90]` so
// a hand-edited value can't silently disable cleanup or balloon the disk.

/// Lower bound. One day is the smallest window that still produces useful
/// hour-level charts.
const LOG_RETENTION_MIN_DAYS: i64 = 1;
/// Upper bound. The Settings page tops out at 90 days; the server enforces
/// the same ceiling so direct DB edits can't set something far larger.
const LOG_RETENTION_MAX_DAYS: i64 = 90;

/// The four preset windows the Settings page offers. The order matters —
/// it's the order the user sees them in.
const LOG_RETENTION_PRESETS: &[i64] = &[7, 14, 30, 90];

/// Read the retention window, falling back to the migration's default if
/// the row is missing or unreadable. Never errors — a cosmetic preference
/// should not fail a request.
pub async fn current_log_retention_days(pool: &sqlx::SqlitePool) -> i64 {
    db::get_setting(pool, "log_retention_days")
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|d| (LOG_RETENTION_MIN_DAYS..=LOG_RETENTION_MAX_DAYS).contains(d))
        .unwrap_or(7)
}

/// GET /api/settings/log-retention-days — admin only.
pub async fn get_log_retention_days(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    let _user = require_admin(&state, &headers)?;
    Ok(Json(json!({
        "days": current_log_retention_days(&state.pool).await,
        "min": LOG_RETENTION_MIN_DAYS,
        "max": LOG_RETENTION_MAX_DAYS,
        "presets": LOG_RETENTION_PRESETS,
    })))
}

#[derive(Deserialize)]
pub struct SetLogRetentionDaysReq {
    /// The new retention window in days. Validated by the handler.
    days: i64,
}

/// PUT /api/settings/log-retention-days — admin only. Rejects out-of-range
/// values so a typo can't silently disable cleanup.
pub async fn set_log_retention_days(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SetLogRetentionDaysReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let _user = require_admin(&state, &headers)
        .map_err(|s| (s, json!({"error": "需要管理员权限"}).into()))?;
    if !(LOG_RETENTION_MIN_DAYS..=LOG_RETENTION_MAX_DAYS).contains(&req.days) {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({
                "error": format!(
                    "保留天数需在 {min}–{max} 之间",
                    min = LOG_RETENTION_MIN_DAYS,
                    max = LOG_RETENTION_MAX_DAYS
                )
            })
            .into(),
        ));
    }
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('log_retention_days', ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(req.days.to_string())
    .execute(&state.pool)
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error": "db error"}).into(),
        )
    })?;
    Ok(Json(json!({ "days": req.days })))
}

// ---------- breaker (circuit breaker) ----------

/// Read one breaker setting, falling back to a hardcoded default. We parse
/// with `unwrap_or(default)` rather than rejecting — a typo in the DB
/// shouldn't take the gateway down.
async fn read_setting(pool: &sqlx::SqlitePool, key: &str, default: &str) -> String {
    db::get_setting(pool, key)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| default.to_string())
}

/// Load all breaker settings from the `settings` table at startup. Missing
/// keys use the `Default` impl — migration 0022 pre-seeds them so a fresh
/// DB works out of the box.
pub async fn load_breaker_config(pool: &sqlx::SqlitePool) -> BreakerConfig {
    let enabled = read_setting(pool, "breaker_enabled", "1").await == "1";
    let base_delay: u64 = read_setting(pool, "breaker_base_delay_secs", "30")
        .await
        .parse()
        .unwrap_or(30);
    let max_delay: u64 = read_setting(pool, "breaker_max_delay_secs", "600")
        .await
        .parse()
        .unwrap_or(600);
    let probe_interval: u64 = read_setting(pool, "breaker_probe_interval_secs", "30")
        .await
        .parse()
        .unwrap_or(30);
    BreakerConfig {
        enabled,
        base_delay: Duration::from_secs(base_delay),
        max_delay: Duration::from_secs(max_delay),
        probe_interval: Duration::from_secs(probe_interval),
    }
}

/// GET /api/settings/breaker — admin only. Returns the in-memory config
/// (which may be newer than the DB if an admin just ran a hot-save).
pub async fn get_breaker_config(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    let _user = require_admin(&state, &headers)?;
    let cfg = state.breaker.config_snapshot().await;
    Ok(Json(json!({
        "enabled": cfg.enabled,
        "base_delay_secs": cfg.base_delay.as_secs(),
        "max_delay_secs": cfg.max_delay.as_secs(),
        "probe_interval_secs": cfg.probe_interval.as_secs(),
    })))
}

#[derive(Deserialize)]
pub struct SetBreakerConfigReq {
    enabled: Option<bool>,
    base_delay_secs: Option<u64>,
    max_delay_secs: Option<u64>,
    probe_interval_secs: Option<u64>,
}

/// PUT /api/settings/breaker — admin only. Each field is optional; absent
/// fields are kept at their current value. Persists all 4 keys to the
/// `settings` table (idempotent UPSERT) and hot-swaps the running config.
pub async fn set_breaker_config(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SetBreakerConfigReq>,
) -> Result<Json<Value>, StatusCode> {
    let _user = require_admin(&state, &headers)?;
    let mut cfg = state.breaker.config_snapshot().await;
    if let Some(v) = req.enabled {
        cfg.enabled = v;
    }
    if let Some(v) = req.base_delay_secs {
        cfg.base_delay = Duration::from_secs(v);
    }
    if let Some(v) = req.max_delay_secs {
        cfg.max_delay = Duration::from_secs(v);
    }
    if let Some(v) = req.probe_interval_secs {
        cfg.probe_interval = Duration::from_secs(v);
    }
    let pairs: [(&str, String); 4] = [
        (
            "breaker_enabled",
            if cfg.enabled { "1".into() } else { "0".into() },
        ),
        (
            "breaker_base_delay_secs",
            cfg.base_delay.as_secs().to_string(),
        ),
        (
            "breaker_max_delay_secs",
            cfg.max_delay.as_secs().to_string(),
        ),
        (
            "breaker_probe_interval_secs",
            cfg.probe_interval.as_secs().to_string(),
        ),
    ];
    for (k, v) in pairs.iter() {
        let _ = sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(k)
        .bind(v)
        .execute(&state.pool)
        .await;
    }
    state.breaker.replace_config(cfg.clone()).await;
    Ok(Json(json!({
        "enabled": cfg.enabled,
        "base_delay_secs": cfg.base_delay.as_secs(),
        "max_delay_secs": cfg.max_delay.as_secs(),
        "probe_interval_secs": cfg.probe_interval.as_secs(),
    })))
}

// ---------- proxy settings ----------

#[derive(Deserialize)]
pub struct ProxySettingsReq {
    /// Proxy host/IP. Empty = no proxy.
    #[serde(default)]
    pub host: String,
    /// Proxy port. 0 = no proxy.
    #[serde(default = "default_zero")]
    pub port: u16,
}
fn default_zero() -> u16 {
    0
}

/// GET /api/settings/proxy — admin only. Returns current proxy configuration.
pub async fn get_proxy_settings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    let _user = require_admin(&state, &headers)?;
    let host: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'proxy_host'")
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten();
    let port: Option<i64> = sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = 'proxy_port'")
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok());
    Ok(Json(json!({
        "host": host.unwrap_or_default(),
        "port": port.unwrap_or(0),
    })))
}

/// PUT /api/settings/proxy — admin only. Updates proxy host/port and rebuilds the proxied client.
pub async fn set_proxy_settings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ProxySettingsReq>,
) -> Result<Json<Value>, StatusCode> {
    let _user = require_admin(&state, &headers)?;

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('proxy_host', ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(req.host.trim())
    .execute(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('proxy_port', ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(req.port.to_string())
    .execute(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Build the full proxy URL and hot-swap the proxied client.
    // Both host and port empty/0 -> no proxy.
    let proxy_url = if !req.host.trim().is_empty() && req.port > 0 {
        Some(format!("http://{}:{}", req.host.trim(), req.port))
    } else {
        None
    };
    state.set_proxied_client(proxy_url.as_deref());

    Ok(Json(json!({ "host": req.host, "port": req.port })))
}
