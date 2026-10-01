//! Tokens, logs, usage and settings — the ownership-scoped admin surface.
//!
//! The recurring theme is **visibility scoping**: a regular user must only
//! ever see rows belonging to tokens they own, across four different
//! endpoints that each build their own `WHERE` clause. A bug in any one of
//! them is a cross-tenant data leak, which is why the isolation is asserted
//! per endpoint rather than once somewhere.

mod support;

use axum::http::StatusCode;
use serde_json::json;
use support::Harness;

/// Two users, each with one token. Returns `(admin_session, bob_session)`.
async fn two_users(h: &Harness) -> (String, String) {
    let admin = support::login(&h.router, "admin").await;
    let bob_id = support::insert_user(h.pool(), "bob", false).await;
    support::insert_token(h.pool(), "bob-token", bob_id).await;
    let bob = support::login(&h.router, "bob").await;
    (admin, bob)
}

async fn insert_log(pool: &sqlx::SqlitePool, token_name: &str, created_at: i64, total: i64) -> i64 {
    sqlx::query(
        "INSERT INTO logs (token_name, model, request_model, channel_name, status_code,
                           created_at, prompt_tokens, completion_tokens, total_tokens, upstream_model)
         VALUES (?,?,?,?,200,?,?,?,?,?)",
    )
    .bind(token_name)
    .bind("gpt-4o")
    .bind("gpt-4o")
    .bind("ch")
    .bind(created_at)
    .bind(total)
    .bind(0)
    .bind(total)
    .bind("gpt-4o")
    .execute(pool)
    .await
    .expect("insert log")
    .last_insert_rowid()
}

// ===================== tokens =====================

#[tokio::test]
async fn a_regular_user_only_lists_their_own_tokens() {
    let h = Harness::with_admin().await;
    let (admin, bob) = two_users(&h).await;
    let admin_id = support::insert_user(h.pool(), "admin2", true).await;
    let _ = admin_id;
    support::insert_token(
        h.pool(),
        "other-token",
        support::insert_user(h.pool(), "carol", false).await,
    )
    .await;

    let (_, body) = support::call_json(&h.router, "GET", "/api/tokens", None, Some(&bob)).await;
    let names: Vec<&str> = body["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["bob-token"]);

    let (_, body) = support::call_json(&h.router, "GET", "/api/tokens", None, Some(&admin)).await;
    assert_eq!(
        body["tokens"].as_array().unwrap().len(),
        2,
        "admin sees every token"
    );
}

#[tokio::test]
async fn the_admin_token_list_labels_the_owner() {
    let h = Harness::with_admin().await;
    let (admin, _) = two_users(&h).await;
    let (_, body) = support::call_json(&h.router, "GET", "/api/tokens", None, Some(&admin)).await;
    let bob_token = body["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "bob-token")
        .unwrap();
    assert_eq!(bob_token["owner"], "bob");
}

#[tokio::test]
async fn a_created_token_is_owned_by_its_creator() {
    let h = Harness::with_admin().await;
    let (_, bob) = two_users(&h).await;
    let (status, body) = support::call_json(
        &h.router,
        "POST",
        "/api/tokens",
        Some(json!({ "name": "fresh" })),
        Some(&bob),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["key"].as_str().unwrap().starts_with("sk-"));
    assert_eq!(body["owner"], "bob");
}

#[tokio::test]
async fn a_non_admin_cannot_mint_a_token_for_someone_else() {
    // `user_id` in the request body is the privilege-escalation vector: without
    // this guard a regular user could create a token charged to the admin.
    let h = Harness::with_admin().await;
    let (_, bob) = two_users(&h).await;
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();

    let (status, _) = support::call_json(
        &h.router,
        "POST",
        "/api/tokens",
        Some(json!({ "name": "sneaky", "user_id": admin_id })),
        Some(&bob),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_admin_can_assign_a_token_to_another_user() {
    let h = Harness::with_admin().await;
    let (admin, _) = two_users(&h).await;
    let bob_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username='bob'")
        .fetch_one(h.pool())
        .await
        .unwrap();

    let (status, body) = support::call_json(
        &h.router,
        "POST",
        "/api/tokens",
        Some(json!({ "name": "gift", "user_id": bob_id })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["owner"], "bob");
}

#[tokio::test]
async fn a_user_cannot_toggle_or_delete_someone_elses_token() {
    let h = Harness::with_admin().await;
    let (_, bob) = two_users(&h).await;
    let carol_id = support::insert_user(h.pool(), "carol", false).await;
    let victim = support::insert_token(h.pool(), "carol-token", carol_id).await;

    let (status, _) = support::call_json(
        &h.router,
        "PUT",
        &format!("/api/tokens/{}", victim.id),
        Some(json!({ "enabled": false })),
        Some(&bob),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = support::call_json(
        &h.router,
        "DELETE",
        &format!("/api/tokens/{}", victim.id),
        None,
        Some(&bob),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let still_there: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tokens WHERE id=?")
        .bind(victim.id)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(still_there, 1);
}

#[tokio::test]
async fn managing_a_nonexistent_token_is_a_404_not_a_403() {
    // 403 would confirm the id exists; 404 reveals nothing.
    let h = Harness::with_admin().await;
    let (_, bob) = two_users(&h).await;
    let (status, _) =
        support::call_json(&h.router, "DELETE", "/api/tokens/424242", None, Some(&bob)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_disabled_token_stops_working_on_the_relay() {
    let h = Harness::with_admin().await;
    let (_, bob) = two_users(&h).await;
    let key: String = sqlx::query_scalar("SELECT key FROM tokens WHERE name='bob-token'")
        .fetch_one(h.pool())
        .await
        .unwrap();

    let (status, _) = support::call_json(&h.router, "GET", "/v1/models", None, Some(&key)).await;
    assert_eq!(status, StatusCode::OK, "enabled token works");

    let id: i64 = sqlx::query_scalar("SELECT id FROM tokens WHERE name='bob-token'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    support::call_json(
        &h.router,
        "PUT",
        &format!("/api/tokens/{id}"),
        Some(json!({ "enabled": false, "rpm_limit": 0, "daily_token_limit": 0 })),
        Some(&bob),
    )
    .await;

    let (status, _) = support::call_json(&h.router, "GET", "/v1/models", None, Some(&key)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_unknown_token_is_rejected() {
    let h = Harness::with_admin().await;
    let (status, _) =
        support::call_json(&h.router, "GET", "/v1/models", None, Some("sk-nope")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_relay_rejects_a_request_with_no_bearer_token() {
    let h = Harness::with_admin().await;
    let (status, _) = support::call_json(&h.router, "GET", "/v1/models", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ===================== logs =====================

#[tokio::test]
async fn a_regular_user_sees_only_logs_for_their_own_tokens() {
    let h = Harness::with_admin().await;
    let (admin, bob) = two_users(&h).await;
    let carol_id = support::insert_user(h.pool(), "carol", false).await;
    support::insert_token(h.pool(), "carol-token", carol_id).await;
    let now = literouter::db::now();
    insert_log(h.pool(), "bob-token", now, 10).await;
    insert_log(h.pool(), "carol-token", now, 20).await;

    let (_, body) = support::call_json(&h.router, "GET", "/api/logs", None, Some(&bob)).await;
    let names: Vec<&str> = body["logs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["token_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["bob-token"]);
    assert_eq!(
        body["total"], 1,
        "the total must not leak the other user's rows"
    );

    let (_, body) = support::call_json(&h.router, "GET", "/api/logs", None, Some(&admin)).await;
    assert_eq!(body["total"], 2);
}

#[tokio::test]
async fn a_user_with_no_tokens_sees_an_empty_log_list_not_an_error() {
    // The visibility clause collapses to `1=0`; an empty `token IN ()` would be
    // a SQL syntax error.
    let h = Harness::with_admin().await;
    let _admin = support::login(&h.router, "admin").await;
    support::insert_user(h.pool(), "bob", false).await;
    let bob = support::login(&h.router, "bob").await;

    let (status, body) = support::call_json(&h.router, "GET", "/api/logs", None, Some(&bob)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 0);
    assert_eq!(body["logs"], json!([]));
}

#[tokio::test]
async fn orphaned_tokens_stay_visible_to_admins_only() {
    // A token whose owner was deleted has user_id NULL. Admins keep seeing its
    // history; a regular user must not be handed a NULL-owned token name.
    let h = Harness::with_admin().await;
    let (admin, bob) = two_users(&h).await;
    sqlx::query("UPDATE tokens SET user_id=NULL WHERE name='bob-token'")
        .execute(h.pool())
        .await
        .unwrap();
    insert_log(h.pool(), "bob-token", literouter::db::now(), 5).await;

    let (_, body) = support::call_json(&h.router, "GET", "/api/logs", None, Some(&bob)).await;
    assert_eq!(
        body["total"], 0,
        "a NULL-owned token is not visible to a regular user"
    );

    let (_, body) = support::call_json(&h.router, "GET", "/api/logs", None, Some(&admin)).await;
    assert_eq!(body["total"], 1);
}

#[tokio::test]
async fn the_log_list_window_excludes_rows_older_than_the_range() {
    let h = Harness::with_admin().await;
    let (admin, _) = two_users(&h).await;
    let now = literouter::db::now();
    insert_log(h.pool(), "bob-token", now, 1).await;
    insert_log(h.pool(), "bob-token", now - 30 * 86400, 1).await; // 30 days old

    // Default range is 1 hour.
    let (_, body) = support::call_json(&h.router, "GET", "/api/logs", None, Some(&admin)).await;
    assert_eq!(body["total"], 1);

    // range=0 means "no time filter".
    let (_, body) =
        support::call_json(&h.router, "GET", "/api/logs?range=0", None, Some(&admin)).await;
    assert_eq!(body["total"], 2);
}

#[tokio::test]
async fn the_pager_total_matches_the_rows_in_the_selected_window() {
    // The count query and the page query build the WHERE clause separately; if
    // they drift the pager lies about how many rows exist.
    let h = Harness::with_admin().await;
    let (admin, _) = two_users(&h).await;
    let now = literouter::db::now();
    for _ in 0..3 {
        insert_log(h.pool(), "bob-token", now, 1).await;
    }
    insert_log(h.pool(), "bob-token", now - 30 * 86400, 1).await;

    let (_, body) = support::call_json(
        &h.router,
        "GET",
        "/api/logs?page=1&size=2",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(
        body["total"], 3,
        "total is the count in-window, not in-table"
    );
    assert_eq!(body["logs"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn the_log_list_falls_back_to_model_when_upstream_model_is_empty() {
    // Rows written before migration 0014 have no upstream_model.
    let h = Harness::with_admin().await;
    let (admin, _) = two_users(&h).await;
    sqlx::query("UPDATE logs SET upstream_model=''")
        .execute(h.pool())
        .await
        .unwrap();
    insert_log(h.pool(), "bob-token", literouter::db::now(), 1).await;

    let (_, body) = support::call_json(&h.router, "GET", "/api/logs", None, Some(&admin)).await;
    assert_eq!(body["logs"][0]["upstream_model"], "gpt-4o");
}

#[tokio::test]
async fn failed_attempts_are_joined_in_without_inflating_the_total() {
    let h = Harness::with_admin().await;
    let (admin, _) = two_users(&h).await;
    let log_id = insert_log(h.pool(), "bob-token", literouter::db::now(), 1).await;
    for (seq, ok) in [(0, 0), (1, 0), (2, 1)] {
        sqlx::query(
            "INSERT INTO log_attempts (log_id, seq, upstream_model, channel_name, status_code, ok)
             VALUES (?,?,?,?,?,?)",
        )
        .bind(log_id)
        .bind(seq)
        .bind("gpt-4o")
        .bind(format!("ch{seq}"))
        .bind(if ok == 1 { 200 } else { 500 })
        .bind(ok)
        .execute(h.pool())
        .await
        .unwrap();
    }

    let (_, body) = support::call_json(&h.router, "GET", "/api/logs", None, Some(&admin)).await;
    assert_eq!(
        body["total"], 1,
        "three attempts is still one client request"
    );
    let row = &body["logs"][0];
    assert_eq!(
        row["failed_attempts"].as_array().unwrap().len(),
        2,
        "only ok=0 hops are listed"
    );
}

#[tokio::test]
async fn breaker_skipped_hops_are_not_counted_as_failures() {
    // `skipped=1` means no HTTP request was made at all; folding those into the
    // "failed N times" badge would misrepresent upstream health.
    let h = Harness::with_admin().await;
    let (admin, _) = two_users(&h).await;
    let log_id = insert_log(h.pool(), "bob-token", literouter::db::now(), 1).await;
    sqlx::query(
        "INSERT INTO log_attempts (log_id, seq, channel_name, status_code, error, ok, skipped)
         VALUES (?,0,'ch',0,'circuit breaker open',0,1)",
    )
    .bind(log_id)
    .execute(h.pool())
    .await
    .unwrap();

    let (_, body) = support::call_json(&h.router, "GET", "/api/logs", None, Some(&admin)).await;
    assert_eq!(body["logs"][0]["failed_attempts"], json!([]));
}

#[tokio::test]
async fn log_detail_is_scoped_the_same_way_as_the_list() {
    let h = Harness::with_admin().await;
    let (admin, bob) = two_users(&h).await;
    let carol_id = support::insert_user(h.pool(), "carol", false).await;
    support::insert_token(h.pool(), "carol-token", carol_id).await;
    let own = insert_log(h.pool(), "bob-token", literouter::db::now(), 1).await;
    let other = insert_log(h.pool(), "carol-token", literouter::db::now(), 1).await;

    let (status, _) = support::call_json(
        &h.router,
        "GET",
        &format!("/api/logs/{own}"),
        None,
        Some(&bob),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 404, not 403 — a 403 would confirm the row exists.
    let (status, _) = support::call_json(
        &h.router,
        "GET",
        &format!("/api/logs/{other}"),
        None,
        Some(&bob),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = support::call_json(
        &h.router,
        "GET",
        &format!("/api/logs/{other}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn log_detail_omits_prompt_and_response_bodies() {
    // Deliberate privacy stance: the log row records routing metadata only.
    let h = Harness::with_admin().await;
    let (admin, _) = two_users(&h).await;
    let log_id = insert_log(h.pool(), "bob-token", literouter::db::now(), 1).await;
    let (_, body) = support::call_json(
        &h.router,
        "GET",
        &format!("/api/logs/{log_id}"),
        None,
        Some(&admin),
    )
    .await;
    let rendered = body.to_string();
    assert!(!rendered.contains("\"prompt_body\""), "{rendered}");
    assert!(!rendered.contains("messages"), "{rendered}");
}

// ===================== usage =====================

#[tokio::test]
async fn usage_totals_are_scoped_per_user() {
    let h = Harness::with_admin().await;
    let (admin, bob) = two_users(&h).await;
    let carol_id = support::insert_user(h.pool(), "carol", false).await;
    support::insert_token(h.pool(), "carol-token", carol_id).await;
    let now = literouter::db::now();
    insert_log(h.pool(), "bob-token", now, 10).await;
    insert_log(h.pool(), "carol-token", now, 99).await;

    let (_, body) = support::call_json(&h.router, "GET", "/api/usage", None, Some(&bob)).await;
    assert_eq!(body["totals"]["total_tokens"], 10);
    assert_eq!(body["by_token"].as_array().unwrap().len(), 1);

    let (_, body) = support::call_json(&h.router, "GET", "/api/usage", None, Some(&admin)).await;
    assert_eq!(body["totals"]["total_tokens"], 109);
    assert_eq!(body["by_token"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn usage_with_no_rows_reports_zeroes_not_nulls() {
    let h = Harness::with_admin().await;
    let (admin, _) = two_users(&h).await;
    let (_, body) = support::call_json(&h.router, "GET", "/api/usage", None, Some(&admin)).await;
    assert_eq!(body["totals"]["requests"], 0);
    assert_eq!(body["totals"]["total_tokens"], 0);
    assert_eq!(body["by_day"], json!([]));
}

#[tokio::test]
async fn failed_requests_land_in_an_empty_channel_bucket() {
    // A request where every candidate failed gets a synthetic winner with an
    // empty `channel_name` (see `proxy.rs`) — deliberately, so the parent row
    // shows "—" instead of blaming the last channel tried. That empty string
    // then groups into its own bucket on the by-channel tab, which is the
    // nameless `4 requests / 0 tokens` row an admin reported seeing.
    let h = Harness::with_admin().await;
    let (admin, _) = two_users(&h).await;
    let now = literouter::db::now();
    insert_log(h.pool(), "bob-token", now, 100).await;
    sqlx::query(
        "INSERT INTO logs (token_name, model, request_model, channel_name, status_code,
                           created_at, prompt_tokens, completion_tokens, total_tokens, upstream_model, error)
         VALUES (?,?,?,?,502,?,0,0,0,?,?)",
    )
    .bind("bob-token")
    .bind("gpt-4o")
    .bind("gpt-4o")
    .bind("")
    .bind(now)
    .bind("gpt-4o")
    .bind("all 1 candidate(s) failed")
    .execute(h.pool())
    .await
    .expect("insert failed log");

    let (_, body) = support::call_json(&h.router, "GET", "/api/usage", None, Some(&admin)).await;
    let by_channel = body["by_channel"].as_array().unwrap();
    assert_eq!(by_channel.len(), 2, "one real channel + the empty bucket");
    let empty = by_channel
        .iter()
        .find(|r| r["key"] == "")
        .expect("the nameless row should be present");
    assert_eq!(empty["requests"], 1);
    assert_eq!(empty["total_tokens"], 0);
    // It still counts as a client request in the totals — the row is real,
    // only the channel attribution is blank.
    assert_eq!(body["totals"]["requests"], 2);
}

#[tokio::test]
async fn the_usage_range_excludes_older_activity() {
    let h = Harness::with_admin().await;
    let (admin, _) = two_users(&h).await;
    let now = literouter::db::now();
    insert_log(h.pool(), "bob-token", now, 5).await;
    insert_log(h.pool(), "bob-token", now - 30 * 86400, 500).await;

    // Default range is 7 days.
    let (_, body) = support::call_json(&h.router, "GET", "/api/usage", None, Some(&admin)).await;
    assert_eq!(body["totals"]["total_tokens"], 5);
    assert_eq!(body["range_days"], 7);
}

#[tokio::test]
async fn the_usage_range_is_clamped_to_a_sane_window() {
    let h = Harness::with_admin().await;
    let (admin, _) = two_users(&h).await;
    for q in ["range=0", "range=-5", "range=100000"] {
        let (_, body) = support::call_json(
            &h.router,
            "GET",
            &format!("/api/usage?{q}"),
            None,
            Some(&admin),
        )
        .await;
        let days = body["range_days"].as_i64().unwrap();
        assert!((1..=90).contains(&days), "{q} -> {days}");
    }
}

// ===================== retention =====================

#[tokio::test]
async fn log_retention_deletes_attempts_before_their_parent_rows() {
    // `log_attempts` has no FK to `logs`, so the child DELETE has to run
    // first or its `log_id IN (SELECT ...)` subquery matches nothing and every
    // attempt row is orphaned forever.
    let h = Harness::with_admin().await;
    let old = literouter::db::now() - 10 * 86400;
    let stale = insert_log(h.pool(), "bob-token", old, 1).await;
    let fresh = insert_log(h.pool(), "bob-token", literouter::db::now(), 1).await;
    for (log_id, seq) in [(stale, 0), (stale, 1), (fresh, 0)] {
        sqlx::query("INSERT INTO log_attempts (log_id, seq, channel_name, ok) VALUES (?,?,'ch',0)")
            .bind(log_id)
            .bind(seq)
            .execute(h.pool())
            .await
            .unwrap();
    }

    let removed = literouter::db::cleanup_old_logs(h.pool(), 7).await.unwrap();
    assert_eq!(removed, 1);

    let orphans: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM log_attempts WHERE log_id NOT IN (SELECT id FROM logs)",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(orphans, 0, "stale attempt rows must not be orphaned");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM log_attempts WHERE log_id=?")
            .bind(fresh)
            .fetch_one(h.pool())
            .await
            .unwrap(),
        1,
        "recent rows are untouched",
    );
}

#[tokio::test]
async fn log_retention_with_nothing_to_remove_is_a_no_op() {
    let h = Harness::with_admin().await;
    assert_eq!(
        literouter::db::cleanup_old_logs(h.pool(), 7).await.unwrap(),
        0
    );
}

// ===================== settings =====================

#[tokio::test]
async fn the_ui_language_persists_and_is_readable() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let (status, _) = support::call_json(
        &h.router,
        "PUT",
        "/api/settings/language",
        Some(json!({ "language": "en-US" })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Readable without a session, because the router needs it before login.
    let (status, body) =
        support::call_json(&h.router, "GET", "/api/settings/language", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["language"], "en-US");

    let (_, body) = support::call_json(&h.router, "GET", "/api/setup-status", None, None).await;
    assert_eq!(body["language"], "en-US");
}

#[tokio::test]
async fn an_unsupported_language_falls_back_to_chinese() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    for bad in ["fr-FR", "", "klingon"] {
        support::call_json(
            &h.router,
            "PUT",
            "/api/settings/language",
            Some(json!({ "language": bad })),
            Some(&admin),
        )
        .await;
        let (_, body) =
            support::call_json(&h.router, "GET", "/api/settings/language", None, None).await;
        assert_eq!(body["language"], "zh-CN", "input {bad:?}");
    }
}

#[tokio::test]
async fn a_missing_setting_reads_as_none() {
    let h = Harness::new().await;
    assert_eq!(
        literouter::db::get_setting(h.pool(), "no_such_key")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn the_breaker_config_round_trips_through_the_settings_table() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let (status, body) = support::call_json(
        &h.router,
        "PUT",
        "/api/settings/breaker",
        Some(json!({ "enabled": false, "base_delay_secs": 45, "probe_interval_secs": 15 })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, body) = support::call_json(
        &h.router,
        "GET",
        "/api/settings/breaker",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(body["enabled"], false);
    assert_eq!(body["base_delay_secs"], 45);

    // And the startup path reads the same values back.
    let cfg = literouter::settings::load_breaker_config(h.pool()).await;
    assert!(!cfg.enabled);
    assert_eq!(cfg.base_delay, std::time::Duration::from_secs(45));
    assert_eq!(cfg.probe_interval, std::time::Duration::from_secs(15));
}

#[tokio::test]
async fn the_breaker_snapshot_endpoint_is_admin_only() {
    // The breaker handlers collapse both "no session" and "not an admin" to
    // 401, unlike `require_admin` elsewhere which distinguishes them. Pinned
    // here so a future change to 403 is a deliberate, visible one.
    let h = Harness::with_admin().await;
    let (_, bob) = two_users(&h).await;
    let (status, _) =
        support::call_json(&h.router, "GET", "/api/breaker/snapshot", None, Some(&bob)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) =
        support::call_json(&h.router, "POST", "/api/breaker/reset", None, Some(&bob)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = support::call_json(
        &h.router,
        "POST",
        "/api/breaker/probe-now",
        None,
        Some(&bob),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn probing_now_with_nothing_open_is_a_no_op() {
    let h = Harness::with_admin().await;
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
    assert_eq!(body["probed"], 0);
    assert_eq!(body["recovered"], 0);
    assert_eq!(body["snapshot"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn a_breaker_reset_clears_every_recorded_key() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    // Only *open* keys appear in the snapshot; a Success clears the entry
    // outright, so a key has to be tripped to be visible.
    h.breaker
        .record(
            &literouter::breaker::breaker_key("ch", "gpt-4o"),
            literouter::breaker::Outcome::Failure("HTTP 500".into()),
        )
        .await;
    let (_, body) = support::call_json(
        &h.router,
        "GET",
        "/api/breaker/snapshot",
        None,
        Some(&admin),
    )
    .await;
    let snap = body["snapshot"].as_array().unwrap();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0]["channel"], "ch");
    assert_eq!(snap[0]["target_model"], "gpt-4o");

    support::call_json(&h.router, "POST", "/api/breaker/reset", None, Some(&admin)).await;
    let (_, body) = support::call_json(
        &h.router,
        "GET",
        "/api/breaker/snapshot",
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(body["snapshot"], json!([]));
}

// ===================== channels: key redaction =====================

#[tokio::test]
async fn the_channel_list_is_admin_only_because_it_carries_the_upstream_key() {
    // `row_channel` returns `api_key` in plaintext on purpose: the admin edit
    // form prefills the field from the list response (Channels.vue). The
    // invariant that actually protects the key is therefore "this route is
    // admin-gated", not "the payload is redacted" — so that is what is
    // asserted here. If the UI ever stops needing the plaintext value, redact
    // the field and move this assertion to the shape of the response.
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let (_, bob) = two_users(&h).await;
    support::insert_channel(h.pool(), "ch", "https://x", "", "gpt-4o", true).await;

    let (status, _) = support::call_json(&h.router, "GET", "/api/channels", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) =
        support::call_json(&h.router, "GET", "/api/channels", None, Some(&bob)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!body.to_string().contains("sk-upstream-secret"));

    // The admin path does return the key, which is what the edit form binds to.
    let (status, body) =
        support::call_json(&h.router, "GET", "/api/channels", None, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["channels"][0]["api_key"], "sk-upstream-secret");
}

#[tokio::test]
async fn channel_administration_is_refused_to_regular_users() {
    let h = Harness::with_admin().await;
    let (_, bob) = two_users(&h).await;
    for (m, uri) in [
        ("GET", "/api/channels"),
        ("GET", "/api/models"),
        ("GET", "/api/mappings"),
    ] {
        let (status, _) = support::call_json(&h.router, m, uri, None, Some(&bob)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{m} {uri}");
    }
}

// ---------- mappings ----------
//
// Duplicate aliases are the last remaining site of the SQLite-constraint-code
// bug: the handlers here compared against the *extended* code "20602" while
// sqlx reports the primary code "2067", so a conflict degraded to a 500.

mod mappings {
    use super::*;

    async fn admin(h: &Harness) -> String {
        support::login(&h.router, "admin").await
    }

    /// Mapping CRUD is admin-only, so every case starts from a seeded admin.
    async fn harness() -> Harness {
        Harness::with_admin().await
    }

    #[tokio::test]
    async fn duplicate_alias_on_create_returns_409() {
        let h = harness().await;
        let s = admin(&h).await;
        support::insert_mapping_any(h.pool(), "gpt-4o", "gpt-4o").await;

        let (status, _) = support::call_json(
            &h.router,
            "POST",
            "/api/mappings",
            Some(json!({ "alias": "gpt-4o", "targets": ["gpt-4o-mini"] })),
            Some(&s),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "duplicate alias must be a 409, not a 500"
        );
    }

    #[tokio::test]
    async fn duplicate_alias_on_rename_returns_409() {
        let h = harness().await;
        let s = admin(&h).await;
        support::insert_mapping_any(h.pool(), "a", "gpt-4o").await;
        let other = support::insert_mapping_any(h.pool(), "b", "gpt-4o").await;

        let (status, _) = support::call_json(
            &h.router,
            "PUT",
            &format!("/api/mappings/{other}"),
            Some(json!({ "alias": "a", "targets": ["gpt-4o-mini"] })),
            Some(&s),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "renaming onto a taken alias must be a 409"
        );
    }

    #[tokio::test]
    async fn a_free_alias_is_still_accepted() {
        // Guards against over-correcting into always-409.
        let h = harness().await;
        let s = admin(&h).await;

        let (status, _) = support::call_json(
            &h.router,
            "POST",
            "/api/mappings",
            Some(json!({ "alias": "fresh", "targets": ["gpt-4o-mini"] })),
            Some(&s),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn renaming_a_mapping_to_its_own_alias_is_allowed() {
        // UPDATE ... SET alias = <same value> still trips the UNIQUE index on
        // SQLite; the client editing only the target list must not be punished.
        let h = harness().await;
        let s = admin(&h).await;
        let id = support::insert_mapping_any(h.pool(), "keep", "gpt-4o").await;

        let (status, _) = support::call_json(
            &h.router,
            "PUT",
            &format!("/api/mappings/{id}"),
            Some(json!({ "alias": "keep", "targets": ["gpt-4o-mini"] })),
            Some(&s),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
}
