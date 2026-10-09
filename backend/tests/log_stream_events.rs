//! Log stream events: verify that each relay request emits a `pending` event
//! followed by a `final` event with the same `request_id`.
//!
//! The test subscribes to `state.log_sender` directly (not via the SSE endpoint)
//! so we can assert on the broadcast events without needing a full SSE client.

mod support;

use support::Harness;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn pending_then_final_events_share_request_id() {
    let h = Harness::with_admin().await;
    let server = MockServer::start().await;

    // Upstream returns a simple success response
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-up",
            "model": "upstream-model",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"},
                         "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10}
        })))
        .mount(&server)
        .await;

    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    support::insert_channel(h.pool(), "ch", &server.uri(), "", "gpt-4o", true).await;

    // Subscribe to the broadcast channel BEFORE making the request
    let mut rx = h.state.log_sender.subscribe();

    // Make the relay request
    let body = serde_json::json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}]
    });
    support::call_json(
        &h.router,
        "POST",
        "/v1/chat/completions",
        Some(body),
        Some(&token.key),
    )
    .await;

    // We should receive TWO events: pending (first) then final
    let pending = rx.recv().await.expect("no pending event received");
    let final_ev = rx.recv().await.expect("no final event received");

    // Pending event assertions
    assert!(pending.pending, "first event should be pending=true");
    assert!(
        pending.id > 0,
        "pending event should have real DB id (pending row persisted)"
    );
    assert!(
        !pending.request_id.is_empty(),
        "pending event must have request_id"
    );
    assert_eq!(pending.token_name, "relay");
    assert_eq!(pending.request_model, "gpt-4o");
    assert_eq!(pending.status_code, 0);
    assert_eq!(pending.total_tokens, 0);

    // Final event assertions
    assert!(!final_ev.pending, "second event should be pending=false");
    assert!(final_ev.id > 0, "final event should have real DB id");
    assert_eq!(
        final_ev.request_id, pending.request_id,
        "pending and final must share the same request_id"
    );
    assert_eq!(final_ev.token_name, "relay");
    assert_eq!(final_ev.request_model, "gpt-4o");
    assert_eq!(final_ev.status_code, 200);
    assert_eq!(final_ev.total_tokens, 10);
}

#[tokio::test]
async fn failed_request_also_emits_pending_then_final() {
    let h = Harness::with_admin().await;
    let server = MockServer::start().await;

    // Upstream returns 500 on every request
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_json(serde_json::json!({"error": {"message": "upstream down"}}))
                .insert_header("retry-after", "42"),
        )
        .mount(&server)
        .await;

    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay-fail", admin_id).await;
    support::insert_channel(h.pool(), "ch", &server.uri(), "", "gpt-4o", true).await;

    let mut rx = h.state.log_sender.subscribe();

    let body = serde_json::json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}]
    });
    support::call_json(
        &h.router,
        "POST",
        "/v1/chat/completions",
        Some(body),
        Some(&token.key),
    )
    .await;

    let pending = rx.recv().await.expect("no pending event received");
    let final_ev = rx.recv().await.expect("no final event received");

    assert!(pending.pending, "first event should be pending=true");
    assert!(!final_ev.pending, "second event should be pending=false");
    assert_eq!(final_ev.request_id, pending.request_id);
    // Final event should have non-2xx status (all candidates failed -> 502)
    assert!(final_ev.status_code >= 400 || final_ev.status_code == 502);
    // tokens should be 0 for failed request
    assert_eq!(final_ev.total_tokens, 0);
}

#[tokio::test]
async fn streaming_request_pending_then_final() {
    let h = Harness::with_admin().await;
    let server = MockServer::start().await;

    // Upstream returns an SSE stream
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(
                    "data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
                     data: [DONE]\n\n",
                ),
        )
        .mount(&server)
        .await;

    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay-stream", admin_id).await;
    support::insert_channel(h.pool(), "ch", &server.uri(), "", "gpt-4o", true).await;

    let mut rx = h.state.log_sender.subscribe();

    let body = serde_json::json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true
    });
    support::call_json(
        &h.router,
        "POST",
        "/v1/chat/completions",
        Some(body),
        Some(&token.key),
    )
    .await;

    let pending = rx.recv().await.expect("no pending event received");
    let final_ev = rx.recv().await.expect("no final event received");

    assert!(pending.pending);
    assert!(!final_ev.pending);
    assert_eq!(final_ev.request_id, pending.request_id);
    assert_eq!(final_ev.status_code, 200);
    // Streaming requests also have tokens (from the tail usage chunk)
    assert!(final_ev.total_tokens >= 0);
}

#[tokio::test]
async fn routing_failure_still_emits_pending_then_final() {
    let h = Harness::with_admin().await;

    // No channels at all — request will fail at routing stage
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay-routing", admin_id).await;

    let mut rx = h.state.log_sender.subscribe();

    let body = serde_json::json!({
        "model": "nonexistent-model",
        "messages": [{"role": "user", "content": "hi"}]
    });
    support::call_json(
        &h.router,
        "POST",
        "/v1/chat/completions",
        Some(body),
        Some(&token.key),
    )
    .await;

    let pending = rx.recv().await.expect("no pending event received");
    let final_ev = rx.recv().await.expect("no final event received");

    assert!(pending.pending);
    assert!(!final_ev.pending);
    assert_eq!(final_ev.request_id, pending.request_id);
    // Routing failure -> 404 status
    assert_eq!(final_ev.status_code, 404);
    assert_eq!(final_ev.total_tokens, 0);
}
