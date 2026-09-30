//! LiteRouter library root.
//!
//! The binary in `main.rs` is a thin shell around this crate: it resolves
//! config from the environment, then calls [`build_state`] and
//! [`build_router`]. Everything testable lives here so integration tests in
//! `tests/` can build a full `Router` with a throwaway SQLite file and drive
//! it through `tower::ServiceExt::oneshot` — no ports, no background tasks,
//! no ambient global state.

pub mod admin;
pub mod auth;
pub mod breaker;
pub mod breaker_probe;
pub mod convert;
pub mod db;
pub mod proxy;
pub mod settings;
pub mod state;
pub mod users;

use axum::routing::{get, post};
use axum::Router;
use sqlx::SqlitePool;
use state::AppState;
use std::sync::Arc;

/// Read persisted global settings and assemble the shared [`AppState`].
///
/// Split out of `main` so tests can build the exact production state graph
/// (settings-loaded, breaker-configured) against a temporary database.
pub async fn build_state(pool: SqlitePool) -> Arc<AppState> {
    let debug_logging = db::get_setting(&pool, "debug_logging")
        .await
        .ok()
        .flatten()
        .map(|v| v == "1")
        .unwrap_or(false);
    proxy::set_debug_logging(debug_logging);
    let breaker_cfg = settings::load_breaker_config(&pool).await;
    let breaker = Arc::new(breaker::Breaker::new(breaker_cfg));
    Arc::new(AppState::new(pool, debug_logging, breaker))
}

/// The full API surface. Deliberately free of I/O beyond the state it is
/// given, so `tests/*_api.rs` can mount it in-process.
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        // relay endpoints (OpenAI compatible)
        .route("/v1/chat/completions", post(proxy::chat_completions))
        .route("/v1/messages", post(proxy::anthropic_messages))
        .route("/v1/models", get(proxy::list_models))
        // auth / setup
        .route("/api/setup-status", get(auth::setup_status))
        .route("/api/setup", post(auth::setup))
        .route("/api/login", post(auth::login))
        .route("/api/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route("/api/password", post(auth::change_password))
        // admin endpoints
        .route("/api/models", get(admin::list_channel_models))
        .route("/api/channels", get(admin::list_channels).post(admin::create_channel))
        .route("/api/channels/fetch-models", post(admin::fetch_models))
        .route("/api/channels/test-model", post(admin::test_model))
        .route(
            "/api/channels/:id",
            axum::routing::put(admin::update_channel).delete(admin::delete_channel),
        )
        .route(
            "/api/channels/:id/models",
            post(admin::update_channel_models),
        )
        .route("/api/tokens", get(admin::list_tokens).post(admin::create_token))
        .route(
            "/api/tokens/:id",
            axum::routing::put(admin::toggle_token).delete(admin::delete_token),
        )
        .route("/api/logs", get(admin::list_logs))
        .route("/api/logs/:id", get(admin::get_log))
        .route("/api/usage", get(admin::usage))
        .route(
            "/api/settings/debug-logging",
            get(settings::get_debug_logging).put(settings::set_debug_logging),
        )
        .route(
            "/api/settings/language",
            get(settings::get_language).put(settings::set_language),
        )
        .route(
            "/api/settings/breaker",
            get(settings::get_breaker_config).put(settings::set_breaker_config),
        )
        .route("/api/breaker/snapshot", get(breaker::http_snapshot))
        .route("/api/breaker/reset", post(breaker::http_reset))
        .route(
            "/api/mappings",
            get(admin::list_mappings).post(admin::create_mapping),
        )
        .route(
            "/api/mappings/:id",
            axum::routing::put(admin::update_mapping)
                .delete(admin::delete_mapping),
        )
        // user management
        .route("/api/users", get(users::list_users).post(users::create_user))
        .route(
            "/api/users/:id",
            axum::routing::put(users::update_user).delete(users::delete_user),
        )
        // The SPA is served from a different origin during development (vite
        // on :5173) and the gateway is called from arbitrary SDK hosts in
        // production, so CORS stays permissive.
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state)
}