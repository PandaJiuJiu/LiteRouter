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

    proxy::set_debug_logging(req.enabled);

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
    normalize_language(db::get_setting(pool, "ui_language").await.ok().flatten().as_deref())
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
    let _user = auth::check_auth(&state, &headers).map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "未登录" })),
        )
    })?;

    if !LANGUAGES.contains(&req.language.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            json!({ "error": format!("不支持的语言：{}", req.language) }).into(),
        ));
    }

    store_language(&state.pool, &req.language).await;
    Ok(Json(json!({ "language": req.language })))
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