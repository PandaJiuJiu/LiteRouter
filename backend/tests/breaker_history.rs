//! `/api/breaker/history` and its filter-options endpoint.
//!
//! The hook side (proxy / probe / admin reset → DB row) is exercised in
//! other suites through the relay itself; this file just asserts that the
//! read endpoints can answer the questions an admin actually asks.

mod support;

use axum::http::StatusCode;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use support::Harness;

/// Insert one breaker event with the given fields. `created_at` is taken
/// verbatim so range filters can be tested without sleeping.
async fn insert_event(
    pool: &SqlitePool,
    channel: &str,
    model: &str,
    event: &str,
    reason: &str,
    backoff_secs: i64,
    created_at: i64,
) -> i64 {
    sqlx::query(
        "INSERT INTO breaker_events
         (channel_name, target_model, event, reason, backoff_secs, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(channel)
    .bind(model)
    .bind(event)
    .bind(reason)
    .bind(backoff_secs)
    .bind(created_at)
    .execute(pool)
    .await
    .expect("insert breaker_events")
    .last_insert_rowid()
}

async fn first_event_row(router: &axum::Router, bearer: &str, query: &str) -> (StatusCode, Value) {
    support::call_json(
        router,
        "GET",
        &format!("/api/breaker/history?{query}"),
        None,
        Some(bearer),
    )
    .await
}

#[tokio::test]
async fn the_history_endpoint_returns_what_was_appended() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let now = literouter::db::now();
    insert_event(h.pool(), "openai", "gpt-4o", "tripped", "HTTP 500", 30, now).await;

    let (status, body) = first_event_row(&h.router, &admin, "page=1&size=20").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["channel_name"], "openai");
    assert_eq!(events[0]["target_model"], "gpt-4o");
    assert_eq!(events[0]["event"], "tripped");
    assert_eq!(events[0]["reason"], "HTTP 500");
    assert_eq!(events[0]["backoff_secs"], 30);
}

#[tokio::test]
async fn events_are_ordered_newest_first() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let now = literouter::db::now();
    insert_event(h.pool(), "ch", "m1", "tripped", "old", 30, now - 60).await;
    insert_event(h.pool(), "ch", "m2", "recovered", "new", 0, now).await;

    let (_, body) = first_event_row(&h.router, &admin, "size=20").await;
    let events = body["events"].as_array().unwrap();
    assert_eq!(events[0]["target_model"], "m2", "newer row must come first");
    assert_eq!(events[1]["target_model"], "m1");
}

#[tokio::test]
async fn the_event_filter_excludes_other_kinds() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let now = literouter::db::now();
    insert_event(h.pool(), "ch", "m", "tripped", "x", 30, now).await;
    insert_event(h.pool(), "ch", "m", "recovered", "y", 0, now).await;

    let (_, body) = first_event_row(&h.router, &admin, "event=recovered").await;
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["event"], "recovered");
}

#[tokio::test]
async fn the_channel_and_model_filters_pin_to_their_pair() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let now = literouter::db::now();
    insert_event(h.pool(), "openai", "gpt-4o", "tripped", "x", 30, now).await;
    insert_event(h.pool(), "openai", "claude-3", "tripped", "y", 30, now).await;
    insert_event(h.pool(), "anthropic", "gpt-4o", "tripped", "z", 30, now).await;

    let (_, body) = first_event_row(&h.router, &admin, "channel=openai&model=gpt-4o").await;
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["channel_name"], "openai");
    assert_eq!(events[0]["target_model"], "gpt-4o");
}

#[tokio::test]
async fn the_range_window_hides_rows_outside_it() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let now = literouter::db::now();
    // Two hours old: dropped by the default 1h window.
    insert_event(h.pool(), "ch", "stale", "tripped", "x", 30, now - 7200).await;
    insert_event(h.pool(), "ch", "fresh", "tripped", "y", 30, now).await;

    let (_, body) = first_event_row(&h.router, &admin, "range=1").await;
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["target_model"], "fresh");

    // `range=0` is "no window" — the stale row comes back too.
    let (_, body) = first_event_row(&h.router, &admin, "range=0").await;
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
}

#[tokio::test]
async fn a_non_admin_call_is_rejected() {
    let h = Harness::with_admin().await;
    let bob_id = support::insert_user(h.pool(), "bob", false).await;
    let _ = support::insert_token(h.pool(), "bob-token", bob_id).await;
    let bob = support::login(&h.router, "bob").await;
    let now = literouter::db::now();
    insert_event(h.pool(), "ch", "m", "tripped", "x", 30, now).await;

    let (status, _) = first_event_row(&h.router, &bob, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_unauthenticated_call_is_rejected() {
    let h = Harness::with_admin().await;
    let (status, _) = first_event_row(&h.router, "", "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn filter_options_return_distinct_values_per_facet_rule() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let now = literouter::db::now();
    // Two channels, two models on openai, one on anthropic.
    insert_event(h.pool(), "openai", "gpt-4o", "tripped", "", 30, now).await;
    insert_event(h.pool(), "openai", "claude-3", "tripped", "", 30, now).await;
    insert_event(h.pool(), "anthropic", "gpt-4o", "tripped", "", 30, now).await;

    // No filters: both channels, both models on offer.
    let (_, body) = support::call_json(
        &h.router,
        "GET",
        "/api/breaker/history/filter-options",
        None,
        Some(&admin),
    )
    .await;
    let channels = body["channel"].as_array().unwrap();
    assert_eq!(channels.len(), 2);
    let models = body["model"].as_array().unwrap();
    assert_eq!(models.len(), 2);
    let events = body["event"].as_array().unwrap();
    assert!(
        events.iter().any(|v| v == &json!("tripped")),
        "event facet carries the closed enum"
    );

    // Picking channel=openai must narrow the model list to openai's two,
    // and the channel dropdown itself should still show both choices.
    let (_, body) = support::call_json(
        &h.router,
        "GET",
        "/api/breaker/history/filter-options?channel=openai",
        None,
        Some(&admin),
    )
    .await;
    let channels = body["channel"].as_array().unwrap();
    assert_eq!(channels.len(), 2, "facet rule: don't drop the picked value");
    let models = body["model"].as_array().unwrap();
    assert_eq!(models.len(), 2, "facet rule: apply the *other* dropdowns");
}
