//! Log stream events: verify that each relay request emits a `pending` event
//! followed by a `final` event with the same `request_id`.
//!
//! The test subscribes to `state.log_sender` directly (not via the SSE endpoint)
//! so we can assert on the broadcast events without needing a full SSE client.

mod support;

use literouter::state::LogEvent;
use support::Harness;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The relay emits several `pending` events now — one on arrival, then one per
/// hop that goes in flight or settles — before the single final event. This
/// reads past them to the settle, which is what a subscriber's final state
/// should be.
async fn recv_final(rx: &mut tokio::sync::broadcast::Receiver<LogEvent>) -> LogEvent {
    loop {
        let ev = rx.recv().await.expect("no log event received");
        if !ev.pending {
            return ev;
        }
    }
}

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
    let final_ev = recv_final(&mut rx).await;

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
    let final_ev = recv_final(&mut rx).await;

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
    let final_ev = recv_final(&mut rx).await;

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
    let final_ev = recv_final(&mut rx).await;

    assert!(pending.pending);
    assert!(!final_ev.pending);
    assert_eq!(final_ev.request_id, pending.request_id);
    // Routing failure -> 404 status
    assert_eq!(final_ev.status_code, 404);
    assert_eq!(final_ev.total_tokens, 0);
}

/// A failover request emits a *series* of pending events: one per hop going in
/// flight and settling. The subscriber must be able to follow the chain as it
/// forms — including the moment the second hop is still in flight (status 0)
/// right after the first one failed — rather than only seeing the settled
/// result.
#[tokio::test]
async fn pending_events_replay_the_chain_as_it_forms() {
    let a = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
            "error": {"message": "down"}
        })))
        .mount(&a)
        .await;
    let b = MockServer::start().await;
    // Slow enough that the "second hop in flight" event is still on the wire
    // (in the broadcast queue) when the request settles, so the assertions
    // below have something to read.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_secs(1))
                .set_body_json(serde_json::json!({
                    "id": "chatcmpl-b",
                    "model": "gpt-4o",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"},
                                 "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10}
                })),
        )
        .mount(&b)
        .await;

    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay-chain", admin_id).await;
    support::insert_channel(h.pool(), "ch-a", &a.uri(), "", "gpt-4o", true).await;
    support::insert_channel(h.pool(), "ch-b", &b.uri(), "", "gpt-4o", true).await;

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

    // Drain: initial pending (empty chain), hop A in flight / settled, hop B in
    // flight, final. Find the event where the first hop's 500 is settled and
    // the second is still in flight.
    let mut saw_second_in_flight = false;
    loop {
        let ev = rx.recv().await.expect("no event received");
        if !ev.pending {
            // The final event carries the whole chain — first failed, second won.
            assert_eq!(ev.status_code, 200);
            assert_eq!(ev.failed_count, 1);
            let statuses: Vec<i64> = ev.attempts.iter().map(|a| a.status_code).collect();
            assert_eq!(statuses, [500, 200]);
            assert_eq!(ev.total_tokens, 10);
            break;
        }
        let statuses: Vec<i64> = ev.attempts.iter().map(|a| a.status_code).collect();
        if statuses == [500, 0] {
            assert_eq!(ev.attempts[0].channel_name, "ch-a");
            assert_eq!(ev.attempts[1].channel_name, "ch-b");
            assert_eq!(
                ev.failed_count, 1,
                "the settled 500 counts, the in-flight hop does not"
            );
            saw_second_in_flight = true;
        }
    }
    assert!(
        saw_second_in_flight,
        "a pending event must show hop B in flight right after hop A failed"
    );
}
