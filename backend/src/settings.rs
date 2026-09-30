use crate::auth::require_admin;
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
/// keys use the `Default` impl — migration 0021 pre-seeds them so a fresh
/// DB works out of the box.
pub async fn load_breaker_config(pool: &sqlx::SqlitePool) -> BreakerConfig {
    let enabled = read_setting(pool, "breaker_enabled", "1").await == "1";
    let base_delay: u64 = read_setting(pool, "breaker_base_delay_secs", "30")
        .await
        .parse()
        .unwrap_or(30);
    let probe_interval: u64 = read_setting(pool, "breaker_probe_interval_secs", "30")
        .await
        .parse()
        .unwrap_or(30);
    BreakerConfig {
        enabled,
        base_delay: Duration::from_secs(base_delay),
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
        "probe_interval_secs": cfg.probe_interval.as_secs(),
    })))
}

#[derive(Deserialize)]
pub struct SetBreakerConfigReq {
    enabled: Option<bool>,
    base_delay_secs: Option<u64>,
    probe_interval_secs: Option<u64>,
}

/// PUT /api/settings/breaker — admin only. Each field is optional; absent
/// fields are kept at their current value. Persists all 3 keys to the
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
    if let Some(v) = req.probe_interval_secs {
        cfg.probe_interval = Duration::from_secs(v);
    }
    let pairs: [(&str, String); 3] = [
        (
            "breaker_enabled",
            if cfg.enabled { "1".into() } else { "0".into() },
        ),
        (
            "breaker_base_delay_secs",
            cfg.base_delay.as_secs().to_string(),
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
        "probe_interval_secs": cfg.probe_interval.as_secs(),
    })))
}