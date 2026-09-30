mod admin;
mod breaker;
mod breaker_probe;
mod convert;
mod auth;
mod db;
mod proxy;
mod settings;
mod state;
mod users;

use axum::routing::{get, post};
use axum::Router;
use state::AppState;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

#[tokio::main]
async fn main() {
    let db_path =
        std::env::var("LITEROUTER_DB").unwrap_or_else(|_| "literouter.db".to_string());
    let pool = db::init_pool(&db_path).await;
    let debug_logging = match db::get_setting(&pool, "debug_logging").await {
        Ok(Some(v)) => v == "1",
        _ => false,
    };
    proxy::set_debug_logging(debug_logging);
    let breaker_cfg = settings::load_breaker_config(&pool).await;
    let breaker = Arc::new(breaker::Breaker::new(breaker_cfg));
    let state = Arc::new(AppState::new(pool, debug_logging, breaker));

    // background: keep the `logs` table bounded — relay traffic is high
    // volume and every row is an INSERT, so without this the DB grows
    // unbounded. retention window is 7 days; sweep runs once on startup
    // and every hour after.
    {
        let pool = state.pool.clone();
        tokio::spawn(async move {
            loop {
                match db::cleanup_old_logs(&pool, 7).await {
                    Ok(n) if n > 0 => println!("log cleanup: removed {} rows older than 7d", n),
                    Ok(_) => {}
                    Err(e) => eprintln!("log cleanup failed: {}", e),
                }
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });
    }

    // background: probe task — every `breaker_probe_interval_secs`, find
    // every (channel, model) whose cooldown has elapsed and send a
    // synthetic ping to the upstream. Recovery is owned by this loop and
    // is independent of user traffic.
    breaker_probe::spawn(state.clone());

    let app = Router::new()
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
        .route("/api/tokens", get(admin::list_tokens).post(admin::create_token))
        .route(
            "/api/tokens/:id",
            axum::routing::put(admin::toggle_token).delete(admin::delete_token),
        )
        .route("/api/logs", get(admin::list_logs))
        .route("/api/logs/:id", get(admin::get_log))
        .route("/api/usage", get(admin::usage))
        .route("/api/settings/debug-logging", get(settings::get_debug_logging).put(settings::set_debug_logging))
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
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    // serve built frontend if a dist directory exists
    // (docker layout: /app/dist; local dev layout: ../frontend/dist)
    let dist = ["./dist", "../frontend/dist"]
        .iter()
        .map(std::path::Path::new)
        .find(|p| p.exists());
    let app = if let Some(dist) = dist {
        // SPA fallback: unknown paths serve index.html so frontend routes
        // like /channels survive a full page refresh
        let index = dist.join("index.html");
        app.fallback_service(
            tower_http::services::ServeDir::new(dist)
                .fallback(tower_http::services::ServeFile::new(index)),
        )
    } else {
        app
    };

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{}", port);
    println!("literouter listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind failed");
    // `into_make_service_with_connect_info` is what puts the peer address in
    // scope for handlers that take `ConnectInfo<SocketAddr>` — without it
    // axum rejects the extractor and the relay can't record a client IP when
    // there's no reverse proxy setting X-Forwarded-For.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .unwrap();
}