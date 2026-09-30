//! User management and the admin invariants around it.
//!
//! Two rules carry the most weight and get the most coverage here:
//!
//!  1. **At least one admin must always remain.** Both the delete path and the
//!     `is_admin -> 0` path are checked, because either one alone leaving a
//!     zero-admin system is an unrecoverable state — the wizard only runs
//!     while `users` is empty.
//!  2. **Deleting a user orphans their tokens, never cascades.** Historical
//!     usage rows survive, so a deleted user's spend still shows in logs.

mod support;

use axum::http::StatusCode;
use serde_json::json;
use sqlx::Row;
use support::Harness;

/// Create a regular (non-admin) user through the API.
async fn create_user(h: &Harness, session: &str, username: &str, password: &str, admin: bool) -> (StatusCode, serde_json::Value) {
    support::call_json(
        &h.router,
        "POST",
        "/api/users",
        Some(json!({ "username": username, "password": password, "is_admin": admin })),
        Some(session),
    )
    .await
}

#[tokio::test]
async fn listing_users_requires_an_admin_session() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    support::insert_user(h.pool(), "bob", false).await;
    let bob = support::login(&h.router, "bob").await;

    let (status, _) = support::call_json(&h.router, "GET", "/api/users", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = support::call_json(&h.router, "GET", "/api/users", None, Some(&bob)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "regular users must not enumerate accounts");

    let (status, body) = support::call_json(&h.router, "GET", "/api/users", None, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = body["users"].as_array().unwrap().iter().map(|u| u["username"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["admin", "bob"]);
}

#[tokio::test]
async fn the_user_list_never_exposes_password_material() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let (_, body) = support::call_json(&h.router, "GET", "/api/users", None, Some(&admin)).await;
    let rendered = body.to_string();
    assert!(!rendered.contains("password_hash"), "{rendered}");
    assert!(!rendered.contains("password_salt"), "{rendered}");
}

#[tokio::test]
async fn creating_a_user_validates_username_and_password() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;

    let (status, _) = create_user(&h, &admin, "   ", "longenough", false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "blank username");

    let (status, _) = create_user(&h, &admin, "shorty", "1234567", false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "password under 8 chars");
}

#[tokio::test]
async fn a_duplicate_username_is_a_conflict() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    assert_eq!(create_user(&h, &admin, "bob", "longenough", false).await.0, StatusCode::OK);
    assert_eq!(create_user(&h, &admin, "bob", "longenough", false).await.0, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_created_user_can_immediately_log_in() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    create_user(&h, &admin, "bob", "bob-password-1", false).await;

    let (status, body) = support::call_json(
        &h.router,
        "POST",
        "/api/login",
        Some(json!({ "username": "bob", "password": "bob-password-1" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_admin"], false);
    assert!(body["session"].as_str().is_some());
}

#[tokio::test]
async fn usernames_are_trimmed_so_whitespace_variants_still_collide() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    assert_eq!(create_user(&h, &admin, "bob", "longenough", false).await.0, StatusCode::OK);
    // "  bob  " trims to "bob", which is taken. Without the trim this would
    // create a second account that looks identical in the UI.
    assert_eq!(create_user(&h, &admin, "  bob  ", "longenough", false).await.0, StatusCode::CONFLICT);
}

#[tokio::test]
async fn the_last_admin_cannot_be_demoted() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let id = sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();

    let (status, body) = support::call_json(
        &h.router,
        "PUT",
        &format!("/api/users/{id}"),
        Some(json!({ "is_admin": false })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let still_admin: i64 = sqlx::query_scalar("SELECT is_admin FROM users WHERE id=?")
        .bind(id)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(still_admin, 1);
}

#[tokio::test]
async fn the_last_admin_cannot_be_deleted() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let id = sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();

    let (status, _) = support::call_json(
        &h.router,
        "DELETE",
        &format!("/api/users/{id}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id=?")
        .bind(id)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn an_admin_can_be_demoted_while_another_admin_remains() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let (_, created) = create_user(&h, &admin, "root2", "longenough", true).await;
    let id = created["id"].as_i64().unwrap();

    let (status, body) = support::call_json(
        &h.router,
        "PUT",
        &format!("/api/users/{id}"),
        Some(json!({ "is_admin": false })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_admin"], false);
}

#[tokio::test]
async fn an_admin_cannot_delete_their_own_account() {
    // Self-deletion would invalidate the session that is making the request.
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let id = sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE username='admin'")
        .fetch_one(h.pool())
        .await
        .unwrap();

    let (status, _) = support::call_json(&h.router, "DELETE", &format!("/api/users/{id}"), None, Some(&admin)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_non_admin_can_be_deleted_by_an_admin() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let (_, created) = create_user(&h, &admin, "bob", "longenough", false).await;
    let id = created["id"].as_i64().unwrap();

    let (status, _) = support::call_json(&h.router, "DELETE", &format!("/api/users/{id}"), None, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id=?")
        .bind(id)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn deleting_a_user_orphans_their_tokens_instead_of_cascading() {
    // The tokens must survive: they anchor historical `logs` rows, which are
    // the only record of what that user spent.
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let bob_id = support::insert_user(h.pool(), "bob", false).await;
    let tok = support::insert_token(h.pool(), "bob-token", bob_id).await;
    let (status, _) = support::call_json(&h.router, "DELETE", &format!("/api/users/{bob_id}"), None, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);

    let row = sqlx::query("SELECT user_id FROM tokens WHERE id=?").bind(tok.id)
        .fetch_one(h.pool()).await.unwrap();
    let owner: Option<i64> = row.get("user_id");
    assert_eq!(owner, None, "token should be orphaned, not deleted");
}

#[tokio::test]
async fn an_admin_can_reset_another_users_password() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    support::insert_user(h.pool(), "bob", false).await;
    let bob_id = sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE username='bob'")
        .fetch_one(h.pool()).await.unwrap();

    let (status, _) = support::call_json(
        &h.router,
        "PUT",
        &format!("/api/users/{bob_id}"),
        Some(json!({ "password": "brand-new-password" })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The old password stops working, the new one starts.
    let (status, _) = support::call_json(
        &h.router,
        "POST",
        "/api/login",
        Some(json!({ "username": "bob", "password": support::PASSWORD })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = support::call_json(
        &h.router,
        "POST",
        "/api/login",
        Some(json!({ "username": "bob", "password": "brand-new-password" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_too_short_reset_password_is_rejected() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    support::insert_user(h.pool(), "bob", false).await;
    let bob_id = sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE username='bob'")
        .fetch_one(h.pool()).await.unwrap();

    let (status, _) = support::call_json(
        &h.router,
        "PUT",
        &format!("/api/users/{bob_id}"),
        Some(json!({ "password": "abc" })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_empty_reset_password_leaves_the_old_one_in_place() {
    // The admin UI sends "" to mean "don't change it" — treat that as a no-op
    // rather than locking the user out with a blank password.
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    support::insert_user(h.pool(), "bob", false).await;
    let bob_id = sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE username='bob'")
        .fetch_one(h.pool()).await.unwrap();

    support::call_json(
        &h.router,
        "PUT",
        &format!("/api/users/{bob_id}"),
        Some(json!({ "password": "" })),
        Some(&admin),
    )
    .await;

    let (status, _) = support::call_json(
        &h.router,
        "POST",
        "/api/login",
        Some(json!({ "username": "bob", "password": support::PASSWORD })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn updating_a_nonexistent_user_is_a_404() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let (status, _) = support::call_json(
        &h.router,
        "PUT",
        "/api/users/9999",
        Some(json!({ "is_admin": true })),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = support::call_json(&h.router, "DELETE", "/api/users/9999", None, Some(&admin)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ===================== session lifecycle =====================

#[tokio::test]
async fn a_session_stops_working_after_logout() {
    let h = Harness::with_admin().await;
    let session = support::login(&h.router, "admin").await;

    let (status, _) = support::call_json(&h.router, "GET", "/api/me", None, Some(&session)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = support::call_json(&h.router, "POST", "/api/logout", None, Some(&session)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = support::call_json(&h.router, "GET", "/api/me", None, Some(&session)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_user_can_change_their_own_password_and_must_use_the_old_one() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let (_, created) = create_user(&h, &admin, "bob", "bob-password-1", false).await;
    let _ = created;
    // Not `support::login` — that helper only knows the fixture password.
    let (_, body) = support::call_json(
        &h.router,
        "POST",
        "/api/login",
        Some(json!({ "username": "bob", "password": "bob-password-1" })),
        None,
    )
    .await;
    let bob = body["session"].as_str().unwrap().to_string();

    let (status, _) = support::call_json(
        &h.router,
        "POST",
        "/api/password",
        Some(json!({ "old_password": "wrong-one", "new_password": "another-password" })),
        Some(&bob),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "wrong current password");

    let (status, body) = support::call_json(
        &h.router,
        "POST",
        "/api/password",
        Some(json!({ "old_password": "bob-password-1", "new_password": "another-password" })),
        Some(&bob),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, _) = support::call_json(
        &h.router,
        "POST",
        "/api/login",
        Some(json!({ "username": "bob", "password": "another-password" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn me_reports_the_callers_identity() {
    let h = Harness::with_admin().await;
    let admin = support::login(&h.router, "admin").await;
    let (status, body) = support::call_json(&h.router, "GET", "/api/me", None, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["username"], "admin");
    assert_eq!(body["is_admin"], true);
}

#[tokio::test]
async fn a_wrong_password_is_rejected_without_saying_which_field_was_wrong() {
    let h = Harness::with_admin().await;
    let (status, body) = support::call_json(
        &h.router,
        "POST",
        "/api/login",
        Some(json!({ "username": "admin", "password": "not-the-password" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "用户名或密码错误");

    // A nonexistent username gets the identical message — no account probing.
    let (_, body2) = support::call_json(
        &h.router,
        "POST",
        "/api/login",
        Some(json!({ "username": "nobody", "password": "not-the-password" })),
        None,
    )
    .await;
    assert_eq!(body2["error"], body["error"]);
}

#[tokio::test]
async fn setup_rejects_a_password_shorter_than_eight_characters() {
    let h = Harness::new().await;
    let (status, _) = support::call_json(
        &h.router,
        "POST",
        "/api/setup",
        Some(json!({ "username": "root", "password": "short" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
