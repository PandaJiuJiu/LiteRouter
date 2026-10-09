//! Relay contract tests: token auth, routing, protocol conversion, and the
//! multi-candidate failover state machine.
//!
//! Upstreams are `wiremock` servers so the whole chain runs for real over
//! loopback — the point is to exercise the actual `reqwest` client, header
//! rewriting, and response classification, none of which a handler-level
//! `oneshot` can reach.
//!
//! The status-code table at the end is the part worth reading first. The relay
//! never forwards an upstream's non-2xx to the client; instead it walks every
//! candidate and then *synthesizes* a status from the shape of the failures it
//! saw. That decision is invisible in any single-candidate test.

mod support;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use sqlx::Row;
use support::Harness;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A mounted upstream returning a fixed OpenAI-style response.
async fn upstream_ok(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-up",
            "model": "upstream-model",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"},
                         "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10}
        })))
        .mount(server)
        .await;
}

/// A mounted upstream returning `status` on every request.
async fn upstream_status(server: &MockServer, status: u16) {
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(status)
                .set_body_json(json!({"error": {"message": "boom"}}))
                .insert_header("retry-after", "42"),
        )
        .mount(server)
        .await;
}

/// A harness with one admin, one `sk-` token, and a channel pointing at
/// `base_url` that advertises `models`.
async fn relay_ready(base_url: &str, models: &str) -> (Harness, String) {
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    support::insert_channel(h.pool(), "ch", base_url, "", models, true).await;
    (h, token.key)
}

async fn chat(h: &Harness, key: &str, model: &str, extra: Value) -> (StatusCode, Value) {
    let mut body = json!({ "model": model, "messages": [{"role": "user", "content": "hi"}] });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    support::call_json(
        &h.router,
        "POST",
        "/v1/chat/completions",
        Some(body),
        Some(key),
    )
    .await
}

// ===================== token auth =====================

#[tokio::test]
async fn a_request_without_a_model_is_rejected_before_any_upstream_call() {
    let server = MockServer::start().await;
    let (h, key) = relay_ready(&server.uri(), "gpt-4o").await;
    let (status, _) = support::call_json(
        &h.router,
        "POST",
        "/v1/chat/completions",
        Some(json!({ "messages": [] })),
        Some(&key),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn a_malformed_body_is_a_400_not_a_500() {
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    let resp = support::call(
        &h.router,
        "POST",
        "/v1/chat/completions",
        None,
        Some(&token.key),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_exhausted_daily_quota_is_rejected_with_429() {
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    sqlx::query("UPDATE tokens SET daily_token_limit=? WHERE id=?")
        .bind(10)
        .bind(token.id)
        .execute(h.pool())
        .await
        .unwrap();
    // Pre-seed usage that already exceeds today's allowance.
    let day = literouter::db::now() / 86400 * 86400;
    sqlx::query(
        "INSERT INTO logs (token_name, model, channel_name, status_code, created_at, total_tokens)
         VALUES ('relay','m','ch',200,?,20)",
    )
    .bind(day + 3600)
    .execute(h.pool())
    .await
    .unwrap();

    let (status, _) = chat(&h, &token.key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

// ===================== happy path =====================

#[tokio::test]
async fn a_successful_relay_forwards_to_the_channel_and_returns_its_body() {
    let server = MockServer::start().await;
    upstream_ok(&server).await;
    let (h, key) = relay_ready(&server.uri(), "gpt-4o").await;

    let (status, body) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["choices"][0]["message"]["content"], "hi");
}

#[tokio::test]
async fn the_channel_key_is_sent_upstream_and_the_client_token_is_not() {
    let server = MockServer::start().await;
    upstream_ok(&server).await;
    let (h, key) = relay_ready(&server.uri(), "gpt-4o").await;
    chat(&h, &key, "gpt-4o", json!({})).await;

    let reqs = server.received_requests().await.unwrap();
    let auth = reqs[0]
        .headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(auth, "Bearer sk-upstream-secret");
    // The downstream `sk-` credential must not travel past the gateway.
    assert!(!auth.contains(&key), "client token leaked upstream");
}

#[tokio::test]
async fn a_mapping_alias_rewrites_the_model_sent_upstream() {
    let server = MockServer::start().await;
    upstream_ok(&server).await;
    let (h, key) = relay_ready(&server.uri(), "gpt-4o-2024-08-06").await;
    support::insert_mapping_any(h.pool(), "my-alias", "gpt-4o-2024-08-06").await;

    let (status, _) = chat(&h, &key, "my-alias", json!({})).await;
    assert_eq!(status, StatusCode::OK);

    let reqs = server.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(sent["model"], "gpt-4o-2024-08-06");
}

#[tokio::test]
async fn the_upstream_body_reaches_the_client_unchanged() {
    // With no conversion needed the relay is a passthrough, not a rebuild.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "x", "vendor_specific_field": {"deep": [1, 2, 3]},
            "choices": [{"message": {"role": "assistant", "content": "ok"}}]
        })))
        .mount(&server)
        .await;
    let (h, key) = relay_ready(&server.uri(), "gpt-4o").await;

    let (_, body) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(body["vendor_specific_field"]["deep"], json!([1, 2, 3]));
}

#[tokio::test]
async fn a_disabled_channel_is_never_routed_to() {
    let server = MockServer::start().await;
    upstream_ok(&server).await;
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    support::insert_channel(h.pool(), "ch", &server.uri(), "", "gpt-4o", false).await;

    let (status, _) = chat(&h, &token.key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn a_model_that_no_channel_serves_is_a_404_that_still_gets_a_log_row() {
    // "this model isn't served anywhere" is exactly what the log page exists
    // to answer, so a routing failure is a loggable client request.
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    support::insert_channel(h.pool(), "ch", "http://127.0.0.1:1", "", "gpt-4o", true).await;

    let (status, body) = chat(&h, &token.key, "unknown-model", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown-model"),
        "{body}"
    );

    support::wait_for_logs(h.pool(), 1, std::time::Duration::from_secs(1)).await;
    let logs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM logs WHERE token_name='relay'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(logs, 1);
}

#[tokio::test]
async fn a_wildcard_channel_matches_any_requested_model() {
    let server = MockServer::start().await;
    upstream_ok(&server).await;
    let (h, key) = relay_ready(&server.uri(), "*").await;
    let (status, _) = chat(&h, &key, "some-brand-new-model", json!({})).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_model_listed_only_in_disabled_models_is_not_routed() {
    // `models` is the routing set; `disabled_models` exists only so the UI can
    // keep showing the card. Routing must never read it.
    let server = MockServer::start().await;
    upstream_ok(&server).await;
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    sqlx::query(
        "INSERT INTO channels (name, base_url, api_key, models, disabled_models, enabled, created_at)
         VALUES ('ch', ?, 'sk-upstream-secret', '', 'old-model,new-model', 1, 1)",
    )
    .bind(server.uri()).execute(h.pool()).await.unwrap();

    let (status, _) = chat(&h, &token.key, "old-model", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

// ===================== failover =====================

/// Two channels serving the same model, in registration order.
async fn two_channels(a: &str, b: &str) -> (Harness, String) {
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    support::insert_channel(h.pool(), "first", a, "", "gpt-4o", true).await;
    support::insert_channel(h.pool(), "second", b, "", "gpt-4o", true).await;
    (h, token.key)
}

#[tokio::test]
async fn a_5xx_from_the_first_channel_fails_over_to_the_second() {
    let a = MockServer::start().await;
    upstream_status(&a, 500).await;
    let b = MockServer::start().await;
    upstream_ok(&b).await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;

    let (status, body) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(a.received_requests().await.unwrap().len(), 1);
    assert_eq!(b.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_4xx_also_triggers_failover() {
    // The relay treats *every* non-2xx as a fallback signal, including 401 and
    // 404 — a channel that is misconfigured or no longer serves a model should
    // not take the request down when another candidate can serve it.
    for code in [400u16, 401, 404, 429, 500, 503] {
        let a = MockServer::start().await;
        upstream_status(&a, code).await;
        let b = MockServer::start().await;
        upstream_ok(&b).await;
        let (h, key) = two_channels(&a.uri(), &b.uri()).await;

        let (status, _) = chat(&h, &key, "gpt-4o", json!({})).await;
        assert_eq!(status, StatusCode::OK, "upstream {code} should fail over");
    }
}

#[tokio::test]
async fn a_transport_failure_fails_over_to_the_next_channel() {
    // Port 1 is closed, so the connection is refused outright.
    let b = MockServer::start().await;
    upstream_ok(&b).await;
    let (h, key) = two_channels("http://127.0.0.1:1", &b.uri()).await;

    let (status, _) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(b.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn the_log_row_records_every_attempt_in_order() {
    let a = MockServer::start().await;
    upstream_status(&a, 500).await;
    let b = MockServer::start().await;
    upstream_ok(&b).await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;
    chat(&h, &key, "gpt-4o", json!({})).await;

    support::wait_for_logs(h.pool(), 1, std::time::Duration::from_secs(1)).await;

    let rows = sqlx::query(
        "SELECT log_id, seq, channel_name, status_code, ok FROM log_attempts ORDER BY seq",
    )
    .fetch_all(h.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "one client request, two upstream hops");
    let log_ids: std::collections::HashSet<i64> =
        rows.iter().map(|r| r.get::<i64, _>("log_id")).collect();
    assert_eq!(
        log_ids.len(),
        1,
        "both hops belong to the same client request"
    );
    assert_eq!(rows[0].get::<String, _>("channel_name"), "first");
    assert_eq!(rows[0].get::<i64, _>("status_code"), 500);
    assert_eq!(rows[0].get::<i64, _>("ok"), 0);
    assert_eq!(rows[1].get::<String, _>("channel_name"), "second");
    assert_eq!(rows[1].get::<i64, _>("ok"), 1);
}

#[tokio::test]
async fn the_parent_log_row_takes_the_winning_attempts_channel_and_tokens() {
    let a = MockServer::start().await;
    upstream_status(&a, 500).await;
    let b = MockServer::start().await;
    upstream_ok(&b).await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;
    chat(&h, &key, "gpt-4o", json!({})).await;

    support::wait_for_logs(h.pool(), 1, std::time::Duration::from_secs(1)).await;

    let row = sqlx::query("SELECT channel_name, status_code, total_tokens, failed_count FROM logs")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("channel_name"), "second");
    assert_eq!(row.get::<i64, _>("status_code"), 200);
    assert_eq!(row.get::<i64, _>("total_tokens"), 10);
    assert_eq!(row.get::<i64, _>("failed_count"), 1);
}

#[tokio::test]
async fn a_multi_target_mapping_fails_over_across_targets() {
    let a = MockServer::start().await;
    upstream_status(&a, 500).await;
    let b = MockServer::start().await;
    upstream_ok(&b).await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;
    support::insert_mapping_targets(
        h.pool(),
        "my-alias",
        &json!([{"channel": "first", "model": "gpt-4o"}, {"channel": "second", "model": "gpt-4o"}]),
    )
    .await;

    let (status, _) = chat(&h, &key, "my-alias", json!({})).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_pinned_target_ignores_channels_the_admin_did_not_choose() {
    let a = MockServer::start().await;
    upstream_ok(&a).await;
    let b = MockServer::start().await;
    upstream_ok(&b).await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;
    support::insert_mapping_targets(
        h.pool(),
        "pinned",
        &json!([{"channel": "second", "model": "gpt-4o"}]),
    )
    .await;

    let (status, _) = chat(&h, &key, "pinned", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(a.received_requests().await.unwrap().len(), 0);
    assert_eq!(b.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_pin_to_a_missing_channel_does_not_fall_back_to_any_channel() {
    // Nothing was *attempted* — the target named no routable channel at all —
    // so this is the "nothing to try" 404 rather than the "everything I tried
    // failed" 502.
    let a = MockServer::start().await;
    upstream_ok(&a).await;
    let (h, key) = relay_ready(&a.uri(), "gpt-4o").await;
    support::insert_mapping_targets(
        h.pool(),
        "pinned",
        &json!([{"channel": "does-not-exist", "model": "gpt-4o"}]),
    )
    .await;

    let (status, body) = chat(&h, &key, "pinned", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("does-not-exist"));
    assert_eq!(a.received_requests().await.unwrap().len(), 0);
}

// ===================== the synthesized status code =====================

/// Three channels that all fail the same way, so the final status is decided
/// purely by the failure shape rather than by which channel happened to win.
async fn all_fail(base: &str) -> (Harness, String) {
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    support::insert_channel(h.pool(), "first", base, "", "gpt-4o", true).await;
    support::insert_channel(h.pool(), "second", base, "", "gpt-4o", true).await;
    (h, token.key)
}

#[tokio::test]
async fn all_rate_limits_surface_as_429_with_the_upstream_retry_after() {
    // SDKs that honor Retry-After can back off correctly, which they cannot do
    // if the gateway flattens this into a 502.
    let a = MockServer::start().await;
    upstream_status(&a, 429).await;
    let b = MockServer::start().await;
    upstream_status(&b, 429).await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/chat/completions",
        Some(json!({ "model": "gpt-4o", "messages": [] })),
        Some(&key),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(resp.headers().get("retry-after").unwrap(), "42");
}

#[tokio::test]
async fn all_transport_failures_surface_as_504() {
    // Nothing upstream even responded, so this is a timeout-shaped problem.
    let (h, key) = all_fail("http://127.0.0.1:1").await;
    let (status, _) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
}

#[tokio::test]
async fn the_breaker_reason_classifies_an_unreachable_upstream() {
    // Port 1 is closed → connection refused. The reason should say *which*
    // kind of unreachable it was, not repeat reqwest's
    // "error sending request for url (…)" boilerplate.
    let (h, key) = all_fail("http://127.0.0.1:1").await;
    chat(&h, &key, "gpt-4o", json!({})).await;

    let rows = h.breaker.snapshot().await;
    // `all_fail` registers two channels against the same dead URL, so both
    // trip independently.
    assert_eq!(rows.len(), 2);
    for row in &rows {
        let reason = &row.reason;
        assert!(
            reason.starts_with("transport: "),
            "expected a transport classification, got {reason:?}"
        );
        assert!(
            !reason.contains("error sending request"),
            "reqwest boilerplate should be stripped, got {reason:?}"
        );
    }
}

#[tokio::test]
async fn a_mix_of_failure_kinds_surfaces_as_502() {
    let a = MockServer::start().await;
    upstream_status(&a, 429).await;
    let b = MockServer::start().await;
    upstream_status(&b, 500).await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/chat/completions",
        Some(json!({ "model": "gpt-4o", "messages": [] })),
        Some(&key),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    // Retry-After is a rate-limit hint; it must not ride along on a 502.
    assert!(resp.headers().get("retry-after").is_none());
}

#[tokio::test]
async fn the_all_failed_error_lists_every_candidate() {
    let a = MockServer::start().await;
    upstream_status(&a, 500).await;
    let b = MockServer::start().await;
    upstream_status(&b, 503).await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;

    let (status, body) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let msg = body["error"]["message"].as_str().unwrap();
    assert!(msg.contains("first"), "{msg}");
    assert!(msg.contains("second"), "{msg}");
    assert!(msg.contains("2 candidate(s)"), "{msg}");
}

#[tokio::test]
async fn an_all_failed_request_still_writes_a_log_row() {
    let a = MockServer::start().await;
    upstream_status(&a, 500).await;
    let b = MockServer::start().await;
    upstream_status(&b, 500).await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;
    chat(&h, &key, "gpt-4o", json!({})).await;

    support::wait_for_logs(h.pool(), 1, std::time::Duration::from_secs(1)).await;
    // The parent row must not name the last failed channel as if it had served
    // the request — an empty channel renders as "—" in the list.
    let row = sqlx::query("SELECT channel_name, status_code FROM logs")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("channel_name"), "");
    assert_eq!(row.get::<i64, _>("status_code"), 502);
}

/// Force `channel`'s breaker for `model` open, so the next request skips it
/// without touching the network.
async fn trip(h: &Harness, channel: &str, model: &str) {
    h.breaker
        .record(
            &literouter::breaker::breaker_key(channel, model),
            literouter::breaker::Outcome::Failure("HTTP 500".into(), None),
        )
        .await;
}

#[tokio::test]
async fn a_breaker_skip_does_not_flatten_a_uniform_429_into_a_502() {
    // A skip is a hop that never reached an upstream, so it is no evidence
    // about how the ones that did answer failed. Letting it sit in the
    // denominator used to turn "every upstream that responded was rate-limited"
    // into a bare 502 — and with it went the Retry-After the client needs.
    let a = MockServer::start().await;
    upstream_status(&a, 429).await;
    let b = MockServer::start().await;
    upstream_status(&b, 429).await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;
    trip(&h, "first", "gpt-4o").await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/chat/completions",
        Some(json!({ "model": "gpt-4o", "messages": [] })),
        Some(&key),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        resp.headers()
            .get("retry-after")
            .map(|v| v.to_str().unwrap()),
        Some("42"),
        "the rate-limit hint is the whole point of choosing 429"
    );
    assert_eq!(
        a.received_requests().await.unwrap().len(),
        0,
        "the skipped channel must still burn no quota"
    );
}

#[tokio::test]
async fn a_breaker_skip_does_not_flatten_uniform_transport_failures_into_a_502() {
    let (h, key) = all_fail("http://127.0.0.1:1").await;
    trip(&h, "first", "gpt-4o").await;
    let (status, _) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
}

#[tokio::test]
async fn a_breaker_skip_alongside_a_genuine_mix_still_surfaces_as_502() {
    // The counterpart to the two above: excluding skips from the denominator
    // must not manufacture a "uniform" verdict out of a real mix.
    let a = MockServer::start().await;
    upstream_status(&a, 429).await;
    let b = MockServer::start().await;
    upstream_status(&b, 500).await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;
    trip(&h, "first", "gpt-4o").await;

    let (status, _) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn an_anthropic_client_gets_an_anthropic_error_envelope() {
    // `/v1/messages` speaks Anthropic, so its failures have to come back in
    // Anthropic's shape: the top-level `"type": "error"` discriminator and an
    // `error.type` drawn from the spec's closed set, which is what SDKs read
    // to decide whether to retry.
    let a = MockServer::start().await;
    upstream_status(&a, 429).await;
    let (h, key) = relay_ready(&a.uri(), "gpt-4o").await;

    let (status, body) = anthropic_chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["type"], "error", "{body}");
    assert_eq!(body["error"]["type"], "rate_limit_error", "{body}");
    assert!(
        body["error"]["message"].as_str().unwrap().contains("429"),
        "{body}"
    );
}

#[tokio::test]
async fn an_openai_client_keeps_the_openai_error_envelope() {
    // The Anthropic reshape must not leak onto the OpenAI-compatible routes,
    // where downstream SDKs are written against the flatter form.
    let a = MockServer::start().await;
    upstream_status(&a, 429).await;
    let (h, key) = relay_ready(&a.uri(), "gpt-4o").await;

    let (status, body) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert!(
        body.get("type").is_none(),
        "no Anthropic discriminator on an OpenAI route: {body}"
    );
    assert_eq!(body["error"]["type"], "literouter_error", "{body}");
}

// ===================== circuit breaker =====================

#[tokio::test]
async fn an_open_breaker_skips_the_upstream_without_sending_a_request() {
    let a = MockServer::start().await;
    upstream_ok(&a).await;
    let (h, key) = relay_ready(&a.uri(), "gpt-4o").await;

    let bkey = literouter::breaker::breaker_key("ch", "gpt-4o");
    h.breaker
        .record(
            &bkey,
            literouter::breaker::Outcome::Failure("HTTP 500".into(), None),
        )
        .await;

    let (status, _) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(
        status,
        StatusCode::BAD_GATEWAY,
        "the only candidate was skipped"
    );
    assert_eq!(
        a.received_requests().await.unwrap().len(),
        0,
        "an open breaker must not burn upstream quota"
    );
}

#[tokio::test]
async fn a_skipped_hop_is_recorded_as_skipped_not_as_a_failure() {
    // `skipped=1` keeps breaker skips out of the "failed N times" badge; they
    // never reached the network, so calling them upstream failures would
    // misrepresent channel health.
    let a = MockServer::start().await;
    upstream_ok(&a).await;
    let (h, key) = relay_ready(&a.uri(), "gpt-4o").await;
    h.breaker
        .record(
            &literouter::breaker::breaker_key("ch", "gpt-4o"),
            literouter::breaker::Outcome::Failure("HTTP 500".into(), None),
        )
        .await;
    chat(&h, &key, "gpt-4o", json!({})).await;

    support::wait_for_logs(h.pool(), 1, std::time::Duration::from_secs(1)).await;

    let row = sqlx::query("SELECT ok, skipped, error FROM log_attempts")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("ok"), 0);
    assert_eq!(row.get::<i64, _>("skipped"), 1);
    assert_eq!(row.get::<String, _>("error"), "circuit breaker open");
}

#[tokio::test]
async fn a_success_clears_the_breaker_for_that_channel_and_model() {
    let a = MockServer::start().await;
    upstream_ok(&a).await;
    let (h, key) = relay_ready(&a.uri(), "gpt-4o").await;
    let bkey = literouter::breaker::breaker_key("ch", "gpt-4o");
    h.breaker
        .record(
            &bkey,
            literouter::breaker::Outcome::Failure("HTTP 500".into(), None),
        )
        .await;
    assert!(!h.breaker.allow(&bkey).await);

    let (status, _) = chat(&h, &key, "gpt-4o", json!({})).await;
    // With the breaker open the request is skipped, so drive one success
    // directly and check the key is gone.
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    h.breaker
        .record(&bkey, literouter::breaker::Outcome::Success)
        .await;
    assert!(h.breaker.allow(&bkey).await);
    assert_eq!(h.breaker.snapshot().await.len(), 0);
}

#[tokio::test]
async fn a_client_400_does_not_trip_the_breaker() {
    // 400/422 are request-body bugs, not upstream ill health. Tripping here
    // would hide routing errors from the admin instead of surfacing them.
    let a = MockServer::start().await;
    upstream_status(&a, 400).await;
    let (h, key) = relay_ready(&a.uri(), "gpt-4o").await;
    let (status, _) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);

    let bkey = literouter::breaker::breaker_key("ch", "gpt-4o");
    assert!(
        h.breaker.allow(&bkey).await,
        "400 must not open the breaker"
    );
}

#[tokio::test]
async fn an_upstream_500_does_trip_the_breaker() {
    let a = MockServer::start().await;
    upstream_status(&a, 500).await;
    let (h, key) = relay_ready(&a.uri(), "gpt-4o").await;
    chat(&h, &key, "gpt-4o", json!({})).await;
    assert!(
        !h.breaker
            .allow(&literouter::breaker::breaker_key("ch", "gpt-4o"))
            .await
    );
}

#[tokio::test]
async fn the_breaker_reason_carries_the_upstreams_own_error_message() {
    // A bare "HTTP 429" tells the admin nothing they can't already infer
    // from the status. The upstream body is drained (it was being discarded)
    // so the panel shows what actually went wrong.
    let a = MockServer::start().await;
    upstream_status(&a, 429).await;
    let (h, key) = relay_ready(&a.uri(), "gpt-4o").await;
    chat(&h, &key, "gpt-4o", json!({})).await;

    let rows = h.breaker.snapshot().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].reason, "HTTP 429: boom");
}

#[tokio::test]
async fn the_error_message_is_not_leaked_to_the_client_or_the_log() {
    // The richer wording is for the admin panel only. `error` on the attempt
    // row also reaches the client as the final synthesized message, so it
    // stays a bare status code.
    let a = MockServer::start().await;
    upstream_status(&a, 500).await;
    let (h, key) = relay_ready(&a.uri(), "gpt-4o").await;
    let (status, body) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(
        !body.to_string().contains("boom"),
        "upstream wording must not reach the client: {body}"
    );

    support::wait_for_logs(h.pool(), 1, std::time::Duration::from_secs(1)).await;
    let err: String = sqlx::query_scalar("SELECT error FROM log_attempts LIMIT 1")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(err, "HTTP 500");
}

#[tokio::test]
async fn an_unreadable_error_body_leaves_the_reason_as_a_bare_status() {
    // No JSON body to read → we still have the code, which is a usable
    // reason. This must not turn into `HTTP 500: ` or a panic.
    let a = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&a)
        .await;
    let (h, key) = relay_ready(&a.uri(), "gpt-4o").await;
    chat(&h, &key, "gpt-4o", json!({})).await;

    let rows = h.breaker.snapshot().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].reason, "HTTP 503");
}

#[tokio::test]
async fn probing_now_clears_a_key_whose_upstream_recovered() {
    // The whole point of the manual button: an admin fixed the upstream and
    // doesn't want to wait out the backoff. The key is tripped with the
    // default 30s cooldown, so a probe at t=0 can only reach it through the
    // manual path (`tracked_keys`), never the ticker's (`expired_keys`).
    let a = MockServer::start().await;
    upstream_ok(&a).await;
    let h = Harness::with_admin().await;
    support::insert_channel(h.pool(), "ch", &a.uri(), "", "gpt-4o", true).await;
    let bkey = literouter::breaker::breaker_key("ch", "gpt-4o");
    h.breaker
        .record(
            &bkey,
            literouter::breaker::Outcome::Failure("HTTP 500".into(), None),
        )
        .await;
    assert_eq!(h.breaker.snapshot().await.len(), 1, "still open pre-probe");

    let admin = support::login(&h.router, "admin").await;
    let (status, body) = support::call_json(
        &h.router,
        "POST",
        "/api/breaker/probe-now",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["probed"], 1);
    assert_eq!(body["recovered"], 1);
    // A recovered key drops out of the snapshot entirely — which is why the
    // response carries a fresh one instead of making the panel re-fetch.
    assert_eq!(body["snapshot"].as_array().unwrap().len(), 0);
    assert!(h.breaker.snapshot().await.is_empty());
}

#[tokio::test]
async fn probing_now_leaves_a_still_broken_key_open_with_a_fresh_backoff() {
    // A manual probe of a genuinely sick upstream isn't a free action: it
    // records another failure and doubles the backoff, exactly as the ticker
    // would have done moments later.
    let a = MockServer::start().await;
    upstream_status(&a, 500).await;
    let h = Harness::with_admin().await;
    support::insert_channel(h.pool(), "ch", &a.uri(), "", "gpt-4o", true).await;
    let bkey = literouter::breaker::breaker_key("ch", "gpt-4o");
    h.breaker
        .record(
            &bkey,
            literouter::breaker::Outcome::Failure("HTTP 500".into(), None),
        )
        .await;

    let admin = support::login(&h.router, "admin").await;
    let (_, body) = support::call_json(
        &h.router,
        "POST",
        "/api/breaker/probe-now",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(body["probed"], 1);
    assert_eq!(body["recovered"], 0);
    let snap = body["snapshot"].as_array().unwrap();
    assert_eq!(snap.len(), 1, "still open");
    // The reason is refreshed from the probe rather than left stale — including
    // the upstream's own wording, which `upstream_status` mounts as "boom".
    assert_eq!(snap[0]["reason"], "HTTP 500: boom");
    // Doubled from the 30s default.
    assert_eq!(snap[0]["cooldown_remaining_secs"], 59);
}

// ===================== protocol conversion =====================

#[tokio::test]
async fn an_anthropic_client_on_an_openai_only_channel_is_converted_upstream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-1", "model": "gpt-4o",
            "choices": [{"message": {"role": "assistant", "content": "hello"},
                         "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 4, "completion_tokens": 2}
        })))
        .mount(&server)
        .await;
    let (h, key) = relay_ready(&server.uri(), "gpt-4o").await;

    let (status, body) = support::call_json(
        &h.router,
        "POST",
        "/v1/messages",
        Some(json!({
            "model": "gpt-4o", "max_tokens": 100, "system": "be brief",
            "messages": [{"role": "user", "content": "hi"}]
        })),
        Some(&key),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The client gets Anthropic's shape back.
    assert_eq!(body["type"], "message");
    assert_eq!(body["content"][0]["text"], "hello");
    assert_eq!(body["stop_reason"], "end_turn");

    // ...and the upstream got OpenAI's.
    let reqs = server.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(sent["messages"][0]["role"], "system");
    assert_eq!(sent["messages"][1]["content"], "hi");
}

#[tokio::test]
async fn an_openai_client_on_an_anthropic_only_channel_is_converted_upstream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_1", "type": "message", "role": "assistant",
            "model": "claude-x", "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "hello"}],
            "usage": {"input_tokens": 4, "output_tokens": 2}
        })))
        .mount(&server)
        .await;
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    // Only the anthropic base_url is filled in.
    support::insert_channel(h.pool(), "ch", "", &server.uri(), "claude-x", true).await;

    let (status, body) = chat(&h, &token.key, "claude-x", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["choices"][0]["message"]["content"], "hello");

    let reqs = server.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(sent["messages"][0]["content"][0]["type"], "text");
    assert_eq!(sent["messages"][0]["content"][0]["text"], "hi");
    // Anthropic auth uses x-api-key, not an Authorization bearer.
    assert!(reqs[0].headers.get("x-api-key").is_some());
}

#[tokio::test]
async fn a_converted_hop_is_labelled_in_the_log_row() {
    let server = MockServer::start().await;
    // Anthropic-only channel, so the upstream speaks /v1/messages and must
    // answer with a Message envelope — an OpenAI-shaped body here would be
    // rejected by the relay's shape check, not converted.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_1", "type": "message", "role": "assistant",
            "model": "claude-x", "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "x"}]
        })))
        .mount(&server)
        .await;
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    support::insert_channel(h.pool(), "ch", "", &server.uri(), "claude-x", true).await;
    chat(&h, &token.key, "claude-x", json!({})).await;

    support::wait_for_logs(h.pool(), 1, std::time::Duration::from_secs(1)).await;

    let row = sqlx::query("SELECT convert, protocol FROM logs")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("protocol"), "openai");
    assert_eq!(row.get::<String, _>("convert"), "to_anthropic");
}

#[tokio::test]
async fn the_anthropic_version_header_is_forwarded_to_the_upstream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_1", "type": "message", "role": "assistant",
            "content": [], "stop_reason": "end_turn"
        })))
        .mount(&server)
        .await;
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    support::insert_channel(h.pool(), "ch", "", &server.uri(), "claude-x", true).await;

    support::call(
        &h.router,
        "POST",
        "/v1/messages",
        Some(json!({ "model": "claude-x", "max_tokens": 10, "messages": [] })),
        Some(&token.key),
    )
    .await;
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(
        reqs[0].headers.get("anthropic-version").unwrap(),
        "2023-06-01"
    );
}

// ===================== upstream thinking =====================

/// A channel speaking Anthropic natively, returning a minimal message.
async fn anthropic_upstream_ok(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_1", "type": "message", "role": "assistant",
            "content": [], "stop_reason": "end_turn"
        })))
        .mount(server)
        .await;
}

/// A harness whose single channel is Anthropic-only.
async fn anthropic_ready(base_url: &str, model: &str) -> (Harness, String) {
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    support::insert_channel(h.pool(), "ch", "", base_url, model, true).await;
    (h, token.key)
}

#[tokio::test]
async fn an_anthropic_upstream_is_told_not_to_think() {
    // The whole reason this exists: Claude Code's auxiliary calls (its
    // Bash-safety classifier among them) have to come back inside a tight
    // `max_tokens`, and a model that thinks unconditionally spends that budget
    // on reasoning and returns an empty message. See `disable_upstream_thinking`.
    let server = MockServer::start().await;
    anthropic_upstream_ok(&server).await;
    let (h, key) = anthropic_ready(&server.uri(), "claude-x").await;

    anthropic_chat(&h, &key, "claude-x", json!({})).await;

    let reqs = server.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(sent["thinking"], json!({ "type": "disabled" }));
}

#[tokio::test]
async fn an_openai_upstream_is_told_not_to_think() {
    let server = MockServer::start().await;
    upstream_ok(&server).await;
    let (h, key) = relay_ready(&server.uri(), "gpt-4o").await;

    chat(&h, &key, "gpt-4o", json!({})).await;

    let reqs = server.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(sent["enable_thinking"], json!(false));
}

#[tokio::test]
async fn a_client_that_asked_for_thinking_keeps_it() {
    // Overriding an explicit request would silently change what the caller
    // billed for and what it sees in the transcript.
    let server = MockServer::start().await;
    anthropic_upstream_ok(&server).await;
    let (h, key) = anthropic_ready(&server.uri(), "claude-x").await;

    anthropic_chat(
        &h,
        &key,
        "claude-x",
        json!({ "thinking": { "type": "enabled", "budget_tokens": 1024 } }),
    )
    .await;

    let reqs = server.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(sent["thinking"]["type"], "enabled");
    assert_eq!(sent["thinking"]["budget_tokens"], 1024);
}

#[tokio::test]
async fn the_flag_reaches_an_upstream_reached_by_conversion() {
    // Protocol conversion rebuilds the body from scratch, so the flag has to
    // be applied after the rebuild — not before it, or it is simply lost.
    let server = MockServer::start().await;
    upstream_ok(&server).await;
    let (h, key) = relay_ready(&server.uri(), "gpt-4o").await;

    anthropic_chat(&h, &key, "gpt-4o", json!({})).await;

    let reqs = server.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(sent["enable_thinking"], json!(false));
}

// ===================== client info =====================

#[tokio::test]
async fn the_client_ip_and_user_agent_are_recorded_on_the_log_row() {
    let server = MockServer::start().await;
    upstream_ok(&server).await;
    let (h, key) = relay_ready(&server.uri(), "gpt-4o").await;

    let resp = support::call_from(
        &h.router,
        "POST",
        "/v1/chat/completions",
        Some(json!({ "model": "gpt-4o", "messages": [] })),
        Some(&key),
        "203.0.113.7:5555".parse().unwrap(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    support::wait_for_logs(h.pool(), 1, std::time::Duration::from_secs(1)).await;

    let row = sqlx::query("SELECT client_ip FROM logs")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("client_ip"), "203.0.113.7");
}

// ===================== /v1/models =====================

#[tokio::test]
async fn the_model_list_is_the_union_of_enabled_channels() {
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    support::insert_channel(h.pool(), "a", "http://x", "", "gpt-4o, gpt-4o-mini", true).await;
    support::insert_channel(h.pool(), "b", "http://x", "", "gpt-4o,claude-x", true).await;
    support::insert_channel(h.pool(), "off", "http://x", "", "hidden-model", false).await;

    let (status, body) =
        support::call_json(&h.router, "GET", "/v1/models", None, Some(&token.key)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "list");
    let ids: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec!["gpt-4o", "gpt-4o-mini", "claude-x"],
        "deduped and in channel order"
    );
}

#[tokio::test]
async fn a_wildcard_channel_contributes_no_enumerable_models() {
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    support::insert_channel(h.pool(), "a", "http://x", "", "*", true).await;

    let (_, body) =
        support::call_json(&h.router, "GET", "/v1/models", None, Some(&token.key)).await;
    assert_eq!(
        body["data"],
        json!([]),
        "a wildcard advertises nothing concrete"
    );
}

#[tokio::test]
async fn a_wildcard_alias_expands_to_the_pinned_channels_model_list() {
    let a = MockServer::start().await;
    upstream_ok(&a).await;
    let (h, key) = relay_ready(&a.uri(), "gpt-4o,gpt-4o-mini").await;
    support::insert_mapping_targets(h.pool(), "any", &json!([{"channel": "ch", "model": "*"}]))
        .await;

    // The client asks for the alias; the wildcard target expands to whatever
    // the pinned channel actually serves.
    let (status, _) = chat(&h, &key, "any", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let reqs = a.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert!(
        sent["model"] == "gpt-4o" || sent["model"] == "gpt-4o-mini",
        "expanded to a real model, got {}",
        sent["model"]
    );
}

#[tokio::test]
async fn a_wildcard_alias_expands_in_the_channels_stored_model_order() {
    let a = MockServer::start().await;
    upstream_ok(&a).await;
    // Deliberately NOT alphabetical: the admin's drag-to-reorder on the models
    // page is only meaningful if this order survives, so the first CSV entry
    // must be the first hop. The UI writes this string verbatim.
    let (h, key) = relay_ready(&a.uri(), "gpt-4o-mini,gpt-4o").await;
    support::insert_mapping_targets(h.pool(), "any", &json!([{"channel": "ch", "model": "*"}]))
        .await;

    let (status, _) = chat(&h, &key, "any", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let reqs = a.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    // reqs[0] is deterministic: the mock answers 200 on the first hop, so
    // there is no failover and only one upstream request.
    assert_eq!(
        sent["model"], "gpt-4o-mini",
        "first hop is the first CSV entry"
    );
}

// ===================== streaming =====================

#[tokio::test]
async fn a_streaming_request_relays_the_upstream_sse_verbatim() {
    let server = MockServer::start().await;
    let sse = concat!(
        "data: {\"id\":\"c\",\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n",
        "data: {\"id\":\"c\",\"choices\":[{\"delta\":{\"content\":\"b\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"id\":\"c\",\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\n",
        "data: [DONE]\n\n"
    );
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse),
        )
        .mount(&server)
        .await;
    let (h, key) = relay_ready(&server.uri(), "gpt-4o").await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/chat/completions",
        Some(json!({ "model": "gpt-4o", "stream": true, "messages": [] })),
        Some(&key),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("\"content\":\"a\""), "{text}");
    assert!(text.trim_end().ends_with("data: [DONE]"), "{text}");
}

#[tokio::test]
async fn a_stream_conversion_succeeds_in_translating_the_event_shapes() {
    // Anthropic client -> OpenAI upstream, both streaming: the client must get
    // Anthropic event names, not OpenAI chunk frames.
    let server = MockServer::start().await;
    let sse = concat!(
        "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"delta\":{\"content\":\"b\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"id\":\"chatcmpl-1\",\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\n",
        "data: [DONE]\n\n"
    );
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse),
        )
        .mount(&server)
        .await;
    let (h, key) = relay_ready(&server.uri(), "gpt-4o").await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/messages",
        Some(json!({ "model": "gpt-4o", "max_tokens": 10, "stream": true, "messages": [] })),
        Some(&key),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("event: message_start"), "{text}");
    assert!(text.contains("event: message_stop"), "{text}");
    assert!(
        !text.contains("chat.completion.chunk"),
        "leaked OpenAI frames: {text}"
    );
}

/// Poll for the log row a streaming request writes when its body is dropped.
/// The row is finalized by a task spawned from `Drop`, so it lands shortly
/// after the body is exhausted rather than synchronously with the response.
/// Now that a pending row (status_code=0) is inserted before streaming starts,
/// we must wait for the row to be finalized (status_code > 0).
async fn await_log_row(pool: &sqlx::SqlitePool) -> sqlx::sqlite::SqliteRow {
    for _ in 0..100 {
        if let Ok(row) = sqlx::query(
            "SELECT stream, total_tokens, status_code FROM logs ORDER BY id DESC LIMIT 1",
        )
        .fetch_one(pool)
        .await
        {
            // Wait for the row to be finalized (status_code > 0 means it's settled).
            if row.get::<i64, _>("status_code") > 0 {
                return row;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("no log row was written for the streamed request");
}

#[tokio::test]
async fn a_streamed_request_records_its_usage_from_the_stream_tail() {
    let server = MockServer::start().await;
    let sse = concat!(
        "data: {\"id\":\"c\",\"choices\":[{\"delta\":{\"content\":\"a\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"id\":\"c\",\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":4}}\n\n",
        "data: [DONE]\n\n"
    );
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse),
        )
        .mount(&server)
        .await;
    let (h, key) = relay_ready(&server.uri(), "gpt-4o").await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/chat/completions",
        Some(json!({ "model": "gpt-4o", "stream": true, "messages": [] })),
        Some(&key),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();

    // The final usage chunk is only visible after the whole stream is read.
    let row = await_log_row(h.pool()).await;
    assert_eq!(row.get::<i64, _>("stream"), 1);
    assert_eq!(row.get::<i64, _>("total_tokens"), 15);
}

// ============ in-stream errors ============
//
// The buffered path shape-checks the body, but a stream is committed the
// moment its headers land: the status line is already on the wire before a
// body byte exists, so there is nothing left to fail over with. The error
// event itself is forwarded — that is how both protocols *define* an in-stream
// failure — but the hop has to stop being counted as a success, or the log
// shows a green 200 and the breaker records Success forever.

/// A harness with one admin, one `sk-` token, and two Anthropic-speaking
/// channels registered in order. Both advertise the same model so the relay
/// walks them in registration order. Used by the stream-failover tests.
async fn two_channels_anthropic(a: &str, b: &str, models: &str) -> (Harness, String) {
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    support::insert_channel(h.pool(), "first", "", a, models, true).await;
    support::insert_channel(h.pool(), "second", "", b, models, true).await;
    (h, token.key)
}
async fn relay_ready_anthropic(base_url_anthropic: &str, models: &str) -> (Harness, String) {
    let h = Harness::with_admin().await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    support::insert_channel(h.pool(), "ch", "", base_url_anthropic, models, true).await;
    (h, token.key)
}

/// An SSE stream whose very first frame is an error event — the failure mode
/// the pre-commit peek exists to catch, so the next candidate can be tried
/// before the client has seen anything.
async fn upstream_stream_first_frame_error(server: &MockServer) {
    let sse = concat!(
        "event: error\n",
        "data: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"provider_unavailable\"}}\n\n"
    );
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse),
        )
        .mount(server)
        .await;
}

/// A 200 SSE stream that opens normally and then gives up mid-flight.
async fn upstream_stream_then_error(server: &MockServer) {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"gen-1\",\"type\":\"message\"}}\n\n",
        "event: error\n",
        "data: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"JSON error injected into SSE stream\",\"error_type\":\"provider_unavailable\"}}\n\n"
    );
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_stream_containing_an_error_event_is_not_logged_as_a_success() {
    let server = MockServer::start().await;
    upstream_stream_then_error(&server).await;
    // Anthropic passthrough, so the error frame the upstream emits is the
    // exact frame the client sees — conversion would otherwise strip it.
    let (h, key) = relay_ready_anthropic(&server.uri(), "gpt-4o").await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/messages",
        Some(json!({
            "model": "gpt-4o", "max_tokens": 32, "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })),
        Some(&key),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        text.contains("event: error") && text.contains("provider_unavailable"),
        "the error event is part of the client's protocol and must reach it: {text}"
    );

    // The log row is written by LogOnEnd::drop, so poll for it. The stream's
    // own verdict lives on `log_attempts` (the winning row); `logs.error`
    // carries the fixed wording.
    let mut winner: Option<(i64, String)> = None;
    for _ in 0..100 {
        if let Ok(r) = sqlx::query(
            "SELECT a.ok, l.error FROM log_attempts a \
             JOIN logs l ON l.id = a.log_id \
             ORDER BY a.id DESC LIMIT 1",
        )
        .fetch_one(h.pool())
        .await
        {
            if r.get::<i64, _>("ok") == 0 {
                winner = Some((r.get::<i64, _>("ok"), r.get::<String, _>("error")));
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let (ok, error) = winner.expect("the streamed request wrote no failed attempt");
    assert_eq!(ok, 0, "a failed stream is not a success");
    assert_eq!(
        error, "upstream error event inside a 200 stream",
        "the row carries the fixed wording; the upstream text is in the capture"
    );
}

#[tokio::test]
async fn a_stream_error_trips_the_breaker_so_the_dead_provider_stops_being_picked() {
    // Without this the relay records Success the moment headers arrive and the
    // provider is never backed off, so every later request walks the same dead
    // channel again.
    let server = MockServer::start().await;
    upstream_stream_then_error(&server).await;
    let (h, key) = relay_ready_anthropic(&server.uri(), "gpt-4o").await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/messages",
        Some(json!({
            "model": "gpt-4o", "max_tokens": 32, "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })),
        Some(&key),
    )
    .await;
    to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();

    let bkey = literouter::breaker::breaker_key("ch", "gpt-4o");
    for _ in 0..100 {
        if !h.breaker.allow(&bkey).await {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let rows = h.breaker.snapshot().await;
    let row = rows
        .iter()
        .find(|r| r.key == bkey)
        .expect("the failed stream must trip the breaker for that key");
    assert!(
        row.reason.contains("JSON error injected into SSE stream"),
        "the panel should carry the upstream's own wording, got {:?}",
        row.reason
    );
    // The `event: error` line lands first and carries no message, so the
    // placeholder must not be locked in — the `data:` line right behind it
    // upgrades it.
    assert!(
        !row.reason.starts_with("upstream error event"),
        "the placeholder must not be the final reason, got {:?}",
        row.reason
    );
}

#[tokio::test]
async fn a_converted_stream_error_is_read_before_the_converter_drops_it() {
    let server = MockServer::start().await;
    // Anthropic-style SSE upstream; the converter (Anthropic → OpenAI) would
    // drop `event: error` lines as unknown types, so detection has to run on
    // the *raw upstream* line before the converter sees it.
    upstream_stream_then_error(&server).await;
    let (h, key) = relay_ready_anthropic(&server.uri(), "gpt-4o").await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/chat/completions",
        Some(json!({
            "model": "gpt-4o", "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })),
        Some(&key),
    )
    .await;
    to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();

    for _ in 0..100 {
        if let Ok(r) = sqlx::query("SELECT a.ok FROM log_attempts a ORDER BY a.id DESC LIMIT 1")
            .fetch_one(h.pool())
            .await
        {
            assert_eq!(r.get::<i64, _>("ok"), 0);
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the converted stream wrote no log row");
}

#[tokio::test]
async fn a_clean_stream_is_still_logged_as_a_success() {
    // The counterpart: the detector must not fire on ordinary content, least
    // of all on a model whose own output happens to talk about errors.
    let server = MockServer::start().await;
    let sse = concat!(
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"error: undefined variable\"}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    );
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse),
        )
        .mount(&server)
        .await;
    let (h, key) = relay_ready_anthropic(&server.uri(), "gpt-4o").await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/messages",
        Some(json!({
            "model": "gpt-4o", "max_tokens": 32, "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })),
        Some(&key),
    )
    .await;
    to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();

    await_log_row(h.pool()).await;
    let row = sqlx::query(
        "SELECT a.ok, l.error, l.status_code, l.client_aborted FROM log_attempts a JOIN logs l ON l.id = a.log_id \
         ORDER BY a.id DESC LIMIT 1",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(row.get::<i64, _>("ok"), 1);
    assert_eq!(row.get::<String, _>("error"), "");
    assert_eq!(
        row.get::<i64, _>("client_aborted"),
        0,
        "a stream that reached its terminator is not a client cancel"
    );
    assert!(
        h.breaker
            .allow(&literouter::breaker::breaker_key("ch", "gpt-4o"))
            .await,
        "a clean stream must leave the breaker closed"
    );
}

/// #3147, pinned: a Claude Code–style caller that gives up on a stream it no
/// longer wants (parallel speculative requests — the winner cancels the rest).
/// The 200 is real and the hop succeeded, but the usage both protocols report
/// in the stream's tail was never read, so the row would otherwise be a
/// silent green 200 worth zero tokens. It must be marked `client_aborted`
/// **without** being marked a failure: `error` stays empty (every failure
/// affordance in the UI keys off it), `ok` stays 1, and the breaker must not
/// back off a provider that behaved perfectly.
#[tokio::test]
async fn a_client_hangup_marks_the_row_without_failing_it() {
    let server = MockServer::start().await;
    // A live-looking stream that is still open for business when the client
    // walks away mid-flight — no terminator yet, upstream still talking.
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"gen-1\",\"type\":\"message\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"working\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\" still working\"}}\n\n"
    );
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse),
        )
        .mount(&server)
        .await;
    let (h, key) = relay_ready_anthropic(&server.uri(), "gpt-4o").await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/messages",
        Some(json!({
            "model": "gpt-4o", "max_tokens": 32, "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })),
        Some(&key),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    // The hang-up itself: the body is dropped unread, which drops the pump,
    // which drops the upstream connection — exactly the axum path a real
    // disconnected client takes.
    drop(resp);

    await_log_row(h.pool()).await;
    let row = sqlx::query(
        "SELECT a.ok, l.error, l.status_code, l.total_tokens, l.client_aborted \
         FROM log_attempts a JOIN logs l ON l.id = a.log_id ORDER BY a.id DESC LIMIT 1",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(
        row.get::<i64, _>("client_aborted"),
        1,
        "the hang-up must be recorded"
    );
    assert_eq!(
        row.get::<i64, _>("ok"),
        1,
        "a client cancel is not a hop failure"
    );
    assert_eq!(
        row.get::<String, _>("error"),
        "",
        "error stays empty: non-empty means a hop failed"
    );
    assert_eq!(row.get::<i64, _>("status_code"), 200);
    assert_eq!(
        row.get::<i64, _>("total_tokens"),
        0,
        "the usage tail was never read — this is the 0 that confused a human"
    );
    assert!(
        h.breaker
            .allow(&literouter::breaker::breaker_key("ch", "gpt-4o"))
            .await,
        "the provider answered fine; a client hanging up must not back it off"
    );
}

/// A stream that stops mid-content, the way a provider whose connection died
/// looks from here: a 200, some perfectly valid frames, then nothing. No
/// `message_stop`, so the client's protocol never gets its ending.
async fn upstream_truncated_stream(server: &MockServer) {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"gen-1\",\"type\":\"message\",\"usage\":{\"input_tokens\":11,\"output_tokens\":0}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n",
        "event: content_block_delta\n",
        "ddata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\" and then nothing\"}}\n\n"
    );
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_stream_cut_off_before_its_terminator_is_not_logged_as_a_success() {
    // The status really is 200 and the client really does receive the bytes —
    // so nothing here can be judged from the response. But the upstream never
    // said it was done, and by status alone that is indistinguishable from a
    // clean stream, which is how a dead provider stays logged green forever.
    let server = MockServer::start().await;
    upstream_truncated_stream(&server).await;
    let (h, key) = relay_ready_anthropic(&server.uri(), "gpt-4o").await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/messages",
        Some(json!({
            "model": "gpt-4o", "max_tokens": 32, "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })),
        Some(&key),
    )
    .await;
    // The status line was committed before the body existed, so the client
    // still gets its 200 — only the verdict changes.
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        text.contains("partial"),
        "the bytes the upstream did send still reach the client: {text}"
    );

    await_log_row(h.pool()).await;
    let row = sqlx::query(
        "SELECT a.ok, l.error, l.status_code, l.client_aborted FROM log_attempts a \
         JOIN logs l ON l.id = a.log_id ORDER BY a.id DESC LIMIT 1",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(
        row.get::<i64, _>("ok"),
        0,
        "a stream with no terminator is not a success"
    );
    assert_eq!(
        row.get::<i64, _>("status_code"),
        200,
        "the client was answered with 200; the row reports what it got"
    );
    assert_eq!(
        row.get::<String, _>("error"),
        "upstream closed the stream without a terminator",
        "fixed wording, same rule as an in-stream error event"
    );
    assert_eq!(
        row.get::<i64, _>("client_aborted"),
        0,
        "the client read to EOF — this cutoff is the upstream's fault, not a hang-up"
    );
}

#[tokio::test]
async fn a_truncated_stream_trips_the_breaker() {
    // The point of calling it a failure: without it the relay records Success
    // the moment headers arrive and this provider is never backed off.
    let server = MockServer::start().await;
    upstream_truncated_stream(&server).await;
    let (h, key) = relay_ready_anthropic(&server.uri(), "gpt-4o").await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/messages",
        Some(json!({
            "model": "gpt-4o", "max_tokens": 32, "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })),
        Some(&key),
    )
    .await;
    to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    await_log_row(h.pool()).await;

    assert!(
        !h.breaker
            .allow(&literouter::breaker::breaker_key("ch", "gpt-4o"))
            .await,
        "a truncated stream must open the breaker so the next request goes elsewhere"
    );
}

#[tokio::test]
async fn usage_reported_before_the_truncation_is_kept() {
    // The counts `message_start` did report are real billable work even
    // though the stream died before the final output count — discarding them
    // would make a failed call look free.
    let server = MockServer::start().await;
    upstream_truncated_stream(&server).await;
    let (h, key) = relay_ready_anthropic(&server.uri(), "gpt-4o").await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/messages",
        Some(json!({
            "model": "gpt-4o", "max_tokens": 32, "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })),
        Some(&key),
    )
    .await;
    to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    await_log_row(h.pool()).await;

    let row = sqlx::query("SELECT prompt_tokens FROM logs ORDER BY id DESC LIMIT 1")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(
        row.get::<i64, _>("prompt_tokens"),
        11,
        "the input count the upstream reported is not thrown away"
    );
}

#[tokio::test]
async fn a_stream_that_opens_with_an_error_frame_fails_over_before_responding() {
    // The pre-commit peek exists for this case: the *first* SSE frame is an
    // error, so the relay can walk to the next candidate before a single byte
    // has been flushed to the client. Without the peek the client would see
    // a 200 with an error event inside and no recovery.
    let a = MockServer::start().await;
    upstream_stream_first_frame_error(&a).await;
    let b = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(concat!(
                    "event: message_start\n",
                    "data: {\"type\":\"message_start\",\"message\":{\"id\":\"gen-2\",\"type\":\"message\"}}\n\n",
                    "event: content_block_delta\n",
                    "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
                    "event: message_stop\n",
                    "data: {\"type\":\"message_stop\"}\n\n",
                )),
        )
        .mount(&b)
        .await;
    let (h, key) = two_channels_anthropic(&a.uri(), &b.uri(), "gpt-4o").await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/messages",
        Some(json!({
            "model": "gpt-4o", "max_tokens": 32, "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })),
        Some(&key),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        text.contains("\"text\":\"ok\""),
        "the client must receive the second channel's content, not the first channel's error: {text}"
    );
    assert!(
        !text.contains("event: error"),
        "the failed first channel's error frame must not reach the client: {text}"
    );
    assert_eq!(
        a.received_requests().await.unwrap().len(),
        1,
        "the first channel was tried"
    );
    assert_eq!(
        b.received_requests().await.unwrap().len(),
        1,
        "the second channel was tried after the first failed"
    );
}

#[tokio::test]
async fn the_pre_commit_peek_records_the_first_channel_as_invalid_body_in_the_log() {
    let a = MockServer::start().await;
    upstream_stream_first_frame_error(&a).await;
    let b = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(concat!(
                    "event: message_start\n",
                    "data: {\"type\":\"message_start\",\"message\":{\"id\":\"gen-2\",\"type\":\"message\"}}\n\n",
                    "event: message_stop\n",
                    "data: {\"type\":\"message_stop\"}\n\n",
                )),
        )
        .mount(&b)
        .await;
    let (h, key) = two_channels_anthropic(&a.uri(), &b.uri(), "gpt-4o").await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/messages",
        Some(json!({
            "model": "gpt-4o", "max_tokens": 32, "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })),
        Some(&key),
    )
    .await;
    to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();

    // The failed hop is recorded inline in relay(), but the winning hop's
    // log_attempts row is written by LogOnEnd::drop, which only fires after
    // the body is dropped (here, at end of the test). Poll until both rows
    // exist.
    let mut rows: Vec<(String, String, i64)> = Vec::new();
    for _ in 0..100 {
        if let Ok(r) =
            sqlx::query_as("SELECT channel_name, error, ok FROM log_attempts ORDER BY id ASC")
                .fetch_all(h.pool())
                .await
        {
            if r.len() == 2 {
                rows = r;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(rows.len(), 2, "both channels should appear in attempts");
    assert_eq!(rows[0].0, "first", "the failed channel is recorded first");
    assert!(
        rows[0]
            .1
            .contains("stream opened with an upstream error event"),
        "the first channel's verdict is the new peek-time wording, got {:?}",
        rows[0].1
    );
    assert_eq!(rows[0].2, 0, "the failed channel is not ok");
    assert_eq!(
        rows[1].0, "second",
        "the winning channel is recorded second"
    );
    assert_eq!(rows[1].2, 1, "the winning channel is ok");
}

#[tokio::test]
async fn a_stream_with_no_event_within_the_peek_deadline_is_committed_anyway() {
    // A provider that stalls before its first event must not be punished: the
    // client still gets a stream, just one with an extra few ms of latency.
    let a = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                // A body that takes longer than the peek deadline to produce
                // its first frame is approximated here by delaying the
                // response. Even a 200 OK with no body yet is enough to
                // exercise the deadline path.
                .set_delay(std::time::Duration::from_millis(200)),
        )
        .mount(&a)
        .await;
    let (h, key) = relay_ready_anthropic(&a.uri(), "gpt-4o").await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/messages",
        Some(json!({
            "model": "gpt-4o", "max_tokens": 32, "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })),
        Some(&key),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
}

// ============ 2xx body validation ============
//
// A relay station whose own backend is down answers `200` with an error
// envelope. Status alone cannot tell that apart from a success, so
// `try_upstream` shape-checks the body and treats a mismatch as a failure —
// which, crucially, means it still fails over.

/// An upstream that answers 200 with a body of the wrong shape.
async fn upstream_bad_200(server: &MockServer) {
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": {"message": "upstream node is down", "code": "internal"}
        })))
        .mount(server)
        .await;
}

async fn anthropic_chat(h: &Harness, key: &str, model: &str, extra: Value) -> (StatusCode, Value) {
    let mut body = json!({
        "model": model, "max_tokens": 64,
        "messages": [{"role": "user", "content": "hi"}]
    });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    support::call_json(&h.router, "POST", "/v1/messages", Some(body), Some(key)).await
}

#[tokio::test]
async fn a_200_carrying_an_error_envelope_fails_over_to_the_next_channel() {
    let a = MockServer::start().await;
    upstream_bad_200(&a).await;
    let b = MockServer::start().await;
    upstream_ok(&b).await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;

    let (status, body) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The client got the second channel's real completion, not the error page.
    assert_eq!(body["choices"][0]["message"]["content"], "hi");

    support::wait_for_logs(h.pool(), 1, std::time::Duration::from_secs(1)).await;
    let row = sqlx::query("SELECT failed_count FROM logs ORDER BY id DESC LIMIT 1")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("failed_count"), 1);
}

#[tokio::test]
async fn an_anthropic_200_that_is_not_a_message_fails_over() {
    let a = MockServer::start().await;
    // Anthropic-only channel, and this is what the failing relay station
    // actually sends back: HTTP 200, JSON, but no Message envelope.
    let (h, key) = {
        let h = Harness::with_admin().await;
        let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
            .fetch_one(h.pool())
            .await
            .unwrap();
        let token = support::insert_token(h.pool(), "relay", admin_id).await;
        support::insert_channel(h.pool(), "first", "", &a.uri(), "claude-x", true).await;
        (h, token.key)
    };
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": {"message": "no capacity", "type": "overloaded_error"}
        })))
        .mount(&a)
        .await;

    let (status, body) = anthropic_chat(&h, &key, "claude-x", json!({})).await;
    // Only one channel, so the request does fail — but it must fail as a
    // gateway error naming the real cause, not as a 200 handed to the client.
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not an Anthropic message"),
        "{body}"
    );
}

#[tokio::test]
async fn every_candidate_returning_a_bad_200_synthesizes_502() {
    let a = MockServer::start().await;
    upstream_bad_200(&a).await;
    let b = MockServer::start().await;
    upstream_bad_200(&b).await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;

    let (status, body) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    let msg = body["error"]["message"].as_str().unwrap();
    // Both channels are named, so the admin can see the whole chain.
    assert!(msg.contains("first"), "{msg}");
    assert!(msg.contains("second"), "{msg}");
    // Both attempts recorded the real upstream status, not a synthesized one.
    support::wait_for_logs(h.pool(), 1, std::time::Duration::from_secs(1)).await;
    let rows = sqlx::query("SELECT status_code, ok, error FROM log_attempts ORDER BY seq ASC")
        .fetch_all(h.pool())
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert_eq!(row.get::<i64, _>("status_code"), 200);
        assert_eq!(row.get::<i64, _>("ok"), 0);
        assert!(row
            .get::<String, _>("error")
            .contains("not an OpenAI completion"));
    }
}

#[tokio::test]
async fn a_bad_200_does_not_count_as_a_rate_limit() {
    // One channel rate-limited, one answering a bad 200. Neither counter is
    // satisfied on its own, so the mix must land on 502 — not 429, which
    // would tell the client to back off when the real problem is a broken
    // relay station.
    let a = MockServer::start().await;
    upstream_status(&a, 429).await;
    let b = MockServer::start().await;
    upstream_bad_200(&b).await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;

    let (status, _) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn a_bad_200_trips_the_breaker_for_that_channel_and_model() {
    let a = MockServer::start().await;
    upstream_bad_200(&a).await;
    let (h, key) = relay_ready(&a.uri(), "gpt-4o").await;

    let (status, _) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let snapshot = h.breaker.snapshot().await;
    assert_eq!(
        snapshot.len(),
        1,
        "the bad 200 should have tripped the breaker"
    );
    let row = snapshot.into_iter().next().unwrap();
    assert_eq!(row.state, "open");
}

#[tokio::test]
async fn a_failed_request_captures_the_upstream_body_without_the_debug_switch() {
    // Debug logging is off by default for the harness's per-state bool.
    // No setup needed: failures always capture regardless of the switch.
    let a = MockServer::start().await;
    upstream_bad_200(&a).await;
    let b = MockServer::start().await;
    upstream_bad_200(&b).await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;

    let (status, _) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);

    support::wait_for_logs(h.pool(), 1, std::time::Duration::from_secs(1)).await;
    let id: i64 = sqlx::query_scalar("SELECT id FROM logs ORDER BY id DESC LIMIT 1")
        .fetch_one(h.pool())
        .await
        .unwrap();
    support::wait_for_debug(h.pool(), id, std::time::Duration::from_secs(1)).await;
    let body = std::fs::read_to_string(
        support::debug_log_dir(h.pool())
            .join(id.to_string())
            .join("resp.json"),
    )
    .unwrap();
    assert!(
        body.contains("upstream node is down"),
        "the captured body should be what the upstream actually sent: {body}"
    );
}

#[tokio::test]
async fn a_successful_request_writes_no_capture_when_the_switch_is_off() {
    // Switch is off by default (the harness's per-state bool), so no setup.
    let server = MockServer::start().await;
    upstream_ok(&server).await;
    let (h, key) = relay_ready(&server.uri(), "gpt-4o").await;

    let (status, _) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    support::wait_for_logs(h.pool(), 1, std::time::Duration::from_secs(1)).await;
    let id: i64 = sqlx::query_scalar("SELECT id FROM logs ORDER BY id DESC LIMIT 1")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert!(!support::debug_log_dir(h.pool())
        .join(id.to_string())
        .exists());
}

#[tokio::test]
async fn the_debug_switch_captures_successful_requests_too() {
    // Flip the per-state bool on this harness's state. The state is not
    // shared with other harnesses, so parallel tests cannot toggle this
    // off mid-flight.
    let h = Harness::with_admin().await;
    h.state
        .debug_logging
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let server = MockServer::start().await;
    upstream_ok(&server).await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    support::insert_channel(h.pool(), "ch", &server.uri(), "", "gpt-4o", true).await;

    chat(&h, &token.key, "gpt-4o", json!({})).await;
    support::wait_for_logs(h.pool(), 1, std::time::Duration::from_secs(1)).await;
    let id: i64 = sqlx::query_scalar("SELECT id FROM logs ORDER BY id DESC LIMIT 1")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let body = std::fs::read_to_string(
        support::debug_log_dir(h.pool())
            .join(id.to_string())
            .join("resp.json"),
    )
    .unwrap();
    assert!(body.contains("chatcmpl-up"), "{body}");
}

#[tokio::test]
async fn a_capture_is_truncated_and_marked_past_the_cap() {
    // Two channels both return 2xx with a shape that fails the relay's body
    // check (`choices[0].message` missing), so the request exhausts every
    // candidate and the relay's "only the final failure is persisted" rule
    // writes one big truncated capture.
    let filler = "y".repeat(literouter::proxy::DEBUG_BODY_MAX + 4096);
    let a = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"delta": filler.clone()}]
        })))
        .mount(&a)
        .await;
    let b = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"delta": filler}]
        })))
        .mount(&b)
        .await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;

    let (status, _) = chat(&h, &key, "gpt-4o", json!({})).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    support::wait_for_logs(h.pool(), 1, std::time::Duration::from_secs(1)).await;
    // The capture of the first (failing) hop is what gets persisted.
    let id: i64 = sqlx::query_scalar("SELECT id FROM logs ORDER BY id DESC LIMIT 1")
        .fetch_one(h.pool())
        .await
        .unwrap();
    support::wait_for_debug(h.pool(), id, std::time::Duration::from_secs(1)).await;
    let dir = support::debug_log_dir(h.pool()).join(id.to_string());
    let meta: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("resp.meta.json")).unwrap())
            .unwrap();
    assert_eq!(meta["truncated"], true);
    assert_eq!(meta["captured"], literouter::proxy::DEBUG_BODY_MAX);
    assert_eq!(
        std::fs::metadata(dir.join("resp.json")).unwrap().len(),
        literouter::proxy::DEBUG_BODY_MAX as u64
    );
}

#[tokio::test]
async fn a_streaming_request_rejects_a_json_200_and_fails_over() {
    // The streaming side of the same failure. We can't buffer a live SSE body
    // to check its shape, but a JSON content-type on a stream request means
    // the upstream sent a whole document instead of an event stream — and
    // that we can reject before a single byte reaches the client.
    let a = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({"error": {"message": "nope"}})),
        )
        .mount(&a)
        .await;
    let b = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n",
                ),
        )
        .mount(&b)
        .await;
    let (h, key) = two_channels(&a.uri(), &b.uri()).await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/chat/completions",
        Some(json!({"model": "gpt-4o", "stream": true, "messages": []})),
        Some(&key),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        !text.contains("\"nope\""),
        "the bad body reached the client: {text}"
    );
    assert!(text.contains("ok"), "{text}");
}

#[tokio::test]
async fn a_passthrough_stream_captures_the_upstream_bytes() {
    // Switch on this harness's own state — each harness owns its bool, so
    // parallel tests can't toggle it off.
    let h = Harness::with_admin().await;
    h.state
        .debug_logging
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let server = MockServer::start().await;
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"gen-1\",\"type\":\"message\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    );
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse),
        )
        .mount(&server)
        .await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    // Anthropic upstream so this hop is a passthrough going through the peek.
    support::insert_channel(h.pool(), "ch", "", &server.uri(), "gpt-4o", true).await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/messages",
        Some(json!({
            "model": "gpt-4o", "max_tokens": 32, "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })),
        Some(&token.key),
    )
    .await;
    to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    await_log_row(h.pool()).await;

    let id: i64 = sqlx::query_scalar("SELECT id FROM logs ORDER BY id DESC LIMIT 1")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let body = std::fs::read_to_string(
        support::debug_log_dir(h.pool())
            .join(id.to_string())
            .join("resp.json"),
    )
    .unwrap();
    // Regression: this file used to come out 0 bytes on every stream, because
    // the pump accumulated into its line-parsing buffer and never the
    // capture. The equality matters beyond containment: the peeked prefix
    // was pushed into the capture twice (once by the relay, once by the
    // pump), and a `contains` check cannot see that — a file with every
    // frame doubled reads like an upstream that sends its envelope twice,
    // which sent one real debugging session down the wrong path.
    assert_eq!(
        body, sse,
        "the capture must be the upstream stream verbatim — no duplicated peek prefix"
    );
}

#[tokio::test]
async fn a_converted_stream_captures_the_upstream_bytes_too() {
    // Anthropic client, OpenAI upstream, success on the only channel with
    // the debug switch on this harness's state. The capture holds the
    // *upstream* (OpenAI) bytes regardless of the translation happening on
    // the way out.
    let h = Harness::with_admin().await;
    h.state
        .debug_logging
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n"),
        )
        .mount(&server)
        .await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let token = support::insert_token(h.pool(), "relay", admin_id).await;
    support::insert_channel(h.pool(), "ch", &server.uri(), "", "gpt-4o", true).await;

    let resp = support::call(
        &h.router,
        "POST",
        "/v1/messages",
        Some(json!({
            "model": "gpt-4o", "max_tokens": 32, "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        })),
        Some(&token.key),
    )
    .await;
    to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    await_log_row(h.pool()).await;

    let id: i64 = sqlx::query_scalar("SELECT id FROM logs ORDER BY id DESC LIMIT 1")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let body = std::fs::read_to_string(
        support::debug_log_dir(h.pool())
            .join(id.to_string())
            .join("resp.json"),
    )
    .unwrap();
    assert!(
        body.contains("[DONE]"),
        "captured stream was empty: {body:?}"
    );
}

// ===================== client disconnect =====================

/// hyper drops the handler future the moment the downstream socket closes.
/// That happens *before* either the buffered `log_request` or a stream's body
/// `Drop` runs, so without `PendingLogGuard` the row pre-created by
/// `insert_pending_log` kept `status_code = 0` ("in progress") until the hourly
/// sweep deleted it. The guard's own Drop settles the row as a client abort.
#[tokio::test]
async fn a_client_that_disconnects_before_the_response_settles_the_log_as_aborted() {
    let server = MockServer::start().await;
    // Slow enough that the client is guaranteed to hang up long before the
    // upstream answers.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(30)))
        .mount(&server)
        .await;
    let (h, key) = relay_ready(&server.uri(), "gpt-4o").await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {key}"))
        .body(Body::from(
            serde_json::to_vec(&json!({
                "model": "gpt-4o", "stream": false,
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .unwrap(),
        ))
        .unwrap();
    let router = h.router.clone();
    let task = tokio::spawn(async move {
        let _ = router.oneshot(req).await;
    });

    // Wait until the relay has persisted its pending row, so aborting here
    // reproduces the real ordering (row exists, response does not) rather than
    // racing the spawn.
    let mut pending = false;
    for _ in 0..200 {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM logs WHERE status_code = 0")
            .fetch_one(h.pool())
            .await
            .unwrap();
        if n > 0 {
            pending = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(pending, "the relay never wrote its pending row");

    task.abort();
    let _ = task.await;

    // The guard settles the row from its Drop, asynchronously.
    let mut settled = false;
    for _ in 0..200 {
        if let Some(row) = sqlx::query(
            "SELECT status_code, client_aborted, error FROM logs WHERE status_code <> 0",
        )
        .fetch_optional(h.pool())
        .await
        .unwrap()
        {
            assert_eq!(row.get::<i64, _>("status_code"), 499);
            assert_eq!(row.get::<i64, _>("client_aborted"), 1);
            assert_eq!(row.get::<String, _>("error"), "client disconnected");
            settled = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        settled,
        "the pending row was never settled after disconnect"
    );
}
