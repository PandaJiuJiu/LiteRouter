//! Smoke test for the harness itself: if this fails, every other integration
//! suite is suspect, so it is deliberately tiny and dependency-free.

mod support;

use axum::http::StatusCode;
use serde_json::json;

#[tokio::test]
async fn empty_database_needs_setup() {
    let h = support::Harness::new().await;
    let (status, body) =
        support::call_json(&h.router, "GET", "/api/setup-status", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["needsSetup"], true);
    assert_eq!(body["authenticated"], false);
    assert_eq!(body["language"], "zh-CN");
}

#[tokio::test]
async fn setup_creates_admin_and_is_idempotent() {
    let h = support::Harness::new().await;
    let (status, body) = support::call_json(
        &h.router,
        "POST",
        "/api/setup",
        Some(json!({ "username": "root", "password": "a-good-password", "language": "en-US" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_admin"], true);
    assert_eq!(body["language"], "en-US");

    // Second call must be rejected — the wizard is a one-shot.
    let (status, _) = support::call_json(
        &h.router,
        "POST",
        "/api/setup",
        Some(json!({ "username": "root2", "password": "a-good-password" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn router_serves_relay_route_unauthenticated() {
    let h = support::Harness::new().await;
    let (status, _) = support::call_json(
        &h.router,
        "POST",
        "/v1/chat/completions",
        Some(json!({ "model": "x", "messages": [] })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
