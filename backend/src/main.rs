mod admin;
mod auth;
mod db;
mod proxy;
mod state;

use axum::routing::{get, post};
use axum::Router;
use state::AppState;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

#[tokio::main]
async fn main() {
    let db_path = std::env::var("LITE_ONE_API_DB").unwrap_or_else(|_| "lite-one-api.db".to_string());
    let pool = db::init_pool(&db_path).await;
    let state = Arc::new(AppState::new(pool));

    let app = Router::new()
        // relay endpoints (OpenAI compatible)
        .route("/v1/chat/completions", post(proxy::chat_completions))
        .route("/v1/messages", post(proxy::anthropic_messages))
        .route("/v1/models", get(proxy::list_models))
        // admin endpoints
        .route("/api/login", post(auth::login))
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
    println!("lite-one-api listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind failed");
    axum::serve(listener, app).await.unwrap();
}
