//! `POST /api/channels/test-model` — the model-card probe.
//!
//! This is the one admin endpoint that talks to a real upstream, so it runs
//! against `wiremock` rather than a `oneshot`. The properties worth locking
//! down are the ones the UI reads back: every protocol entry must carry an
//! `ok` flag *and* a `ms` latency (the card shows the latency instead of the
//! word "Available"), and a failed probe must carry the upstream's own message
//! so the tooltip has something to show.
//!
//! The timeout comes from the `model_test_timeout_secs` setting, so the
//! timeout cases below set it to 1s rather than paying the production default.

mod support;

use axum::http::StatusCode;
use serde_json::json;
use sqlx::SqlitePool;
use std::time::Duration;
use support::Harness;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// An upstream that answers the OpenAI probe with 200.
async fn openai_ok(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "."}}]
        })))
        .mount(server)
        .await;
}

async fn admin(h: &Harness) -> String {
    support::login(&h.router, "admin").await
}

/// Probe `model` on a channel offering only the OpenAI-compatible URL.
async fn probe(
    h: &Harness,
    session: &str,
    base_url: &str,
    model: &str,
) -> (StatusCode, serde_json::Value) {
    support::call_json(
        &h.router,
        "POST",
        "/api/channels/test-model",
        Some(json!({
            "base_url": base_url,
            "base_url_anthropic": "",
            "api_key": "sk-upstream-secret",
            "model": model,
        })),
        Some(session),
    )
    .await
}

#[tokio::test]
async fn a_reachable_model_reports_ok_with_a_latency() {
    let server = MockServer::start().await;
    openai_ok(&server).await;
    let h = Harness::with_admin().await;
    let s = admin(&h).await;

    let (status, body) = probe(&h, &s, &server.uri(), "gpt-4o").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    let entry = &body["protocols"]["openai"];
    assert_eq!(entry["ok"], true);
    // The card renders the latency rather than a bare "Available", so `ms`
    // has to be a number the UI can format — absent here it would show "—".
    assert!(entry["ms"].is_u64(), "expected a latency, got {entry}");
}

#[tokio::test]
async fn a_failing_probe_keeps_the_upstream_message_and_still_reports_a_latency() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"error": {"message": "invalid api key"}})),
        )
        .mount(&server)
        .await;
    let h = Harness::with_admin().await;
    let s = admin(&h).await;

    let (status, body) = probe(&h, &s, &server.uri(), "gpt-4o").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false);
    let entry = &body["protocols"]["openai"];
    assert_eq!(entry["ok"], false);
    assert!(entry["ms"].is_u64(), "expected a latency, got {entry}");
    // What the tooltip shows. The upstream's own wording, not a generic one —
    // a wrong key and a nonexistent model need different fixes.
    let err = entry["error"].as_str().expect("error");
    assert!(err.contains("401"), "{err}");
    assert!(err.contains("invalid api key"), "{err}");
}

#[tokio::test]
async fn an_unreachable_upstream_is_a_failure_not_an_error() {
    // A dead host must not take the whole endpoint down: the card shows the
    // failure inline and the other protocols still get judged.
    let h = Harness::with_admin().await;
    let s = admin(&h).await;

    // Port 1 on localhost: nothing listens, so this fails at connect time.
    let (status, body) = probe(&h, &s, "http://127.0.0.1:1", "gpt-4o").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false);
    assert_eq!(body["protocols"]["openai"]["ok"], false);
    assert!(body["protocols"]["openai"]["ms"].is_u64());
}

#[tokio::test]
async fn the_probe_is_admin_only() {
    // It carries an `api_key` straight to the upstream, so an unprivileged
    // caller must not be able to use it as a relay to arbitrary hosts.
    let h = Harness::with_admin().await;
    support::insert_user(h.pool(), "bob", false).await;
    let bob = support::login(&h.router, "bob").await;
    let server = MockServer::start().await;
    openai_ok(&server).await;

    let (status, _) = probe(&h, &bob, &server.uri(), "gpt-4o").await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_empty_model_is_rejected_before_any_request_is_sent() {
    let h = Harness::with_admin().await;
    let s = admin(&h).await;

    let (status, _) = probe(&h, &s, "https://example.invalid", "  ").await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------- the timeout ----------

async fn set_timeout(pool: &SqlitePool, secs: &str) {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('model_test_timeout_secs', ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(secs)
    .execute(pool)
    .await
    .expect("write setting");
}

/// An upstream that answers, but only after `delay` — a hung provider.
async fn openai_slow(server: &MockServer, delay: Duration) {
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(delay))
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_slow_upstream_times_out_instead_of_hanging_the_ui() {
    // The whole point of the cap: a hung upstream used to leave the card
    // spinning for the client's full 600s. The error text names the budget
    // so the admin can tell a timeout from a rejection.
    let server = MockServer::start().await;
    openai_slow(&server, Duration::from_secs(30)).await;
    let h = Harness::with_admin().await;
    set_timeout(h.pool(), "1").await;
    let s = admin(&h).await;

    let (status, body) = probe(&h, &s, &server.uri(), "gpt-4o").await;

    assert_eq!(status, StatusCode::OK);
    let entry = &body["protocols"]["openai"];
    assert_eq!(entry["ok"], false);
    assert!(entry["error"].as_str().unwrap().contains("超时"));
    assert!(entry["error"].as_str().unwrap().contains('1'), "{entry}");
    assert!(entry["ms"].is_u64());
}

#[tokio::test]
async fn the_timeout_falls_back_to_the_default_when_the_setting_is_unusable() {
    // A typo in the settings table must not disable testing altogether: the
    // probe still runs, just on the default budget. "0" is the sharp edge —
    // taken literally it would time out every request instantly.
    let server = MockServer::start().await;
    openai_ok(&server).await;
    let h = Harness::with_admin().await;
    set_timeout(h.pool(), "0").await;
    let s = admin(&h).await;

    let (status, body) = probe(&h, &s, &server.uri(), "gpt-4o").await;

    assert_eq!(status, StatusCode::OK);
    // Fell back to 10s, so a prompt upstream still passes.
    assert_eq!(body["protocols"]["openai"]["ok"], true);
}

#[tokio::test]
async fn a_missing_setting_row_still_probes_normally() {
    let server = MockServer::start().await;
    openai_ok(&server).await;
    let h = Harness::with_admin().await;
    let s = admin(&h).await;

    let (status, body) = probe(&h, &s, &server.uri(), "gpt-4o").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["protocols"]["openai"]["ok"], true);
}
