//! Small public helpers with real edge-case behaviour: password verification
//! and language normalization. Both are the kind of function that is one
//! `unwrap()` away from a 500 on user input.

use literouter::db;
use literouter::settings::normalize_language;

// ===================== passwords =====================

#[test]
fn a_freshly_hashed_password_verifies() {
    let (hash, salt) = db::hash_password("correct horse battery staple");
    assert!(db::verify_password("correct horse battery staple", &hash, &salt));
}

#[test]
fn hashing_is_salted_so_two_identical_passwords_differ_on_disk() {
    // Without a per-user salt, two users choosing the same password would be
    // trivially linkable by anyone who read the table.
    let (h1, s1) = db::hash_password("same-password");
    let (h2, s2) = db::hash_password("same-password");
    assert_ne!(h1, h2, "hashes must differ");
    assert_ne!(s1, s2, "salts must differ");
    // ...and both still verify against their own salt.
    assert!(db::verify_password("same-password", &h1, &s1));
    assert!(db::verify_password("same-password", &h2, &s2));
}

#[test]
fn the_stored_formats_are_the_widths_the_schema_documents() {
    let (hash, salt) = db::hash_password("x");
    assert_eq!(hash.len(), 64, "32 bytes hex-encoded");
    assert_eq!(salt.len(), 32, "16 bytes hex-encoded");
}

#[test]
fn a_wrong_password_does_not_verify() {
    let (hash, salt) = db::hash_password("right");
    assert!(!db::verify_password("wrong", &hash, &salt));
    assert!(!db::verify_password("right ", &hash, &salt));
    assert!(!db::verify_password("", &hash, &salt));
}

#[test]
fn a_hash_and_salt_from_different_passwords_do_not_verify() {
    let (h, _) = db::hash_password("a");
    let (_, s) = db::hash_password("b");
    assert!(!db::verify_password("a", &h, &s));
}

#[test]
fn malformed_hex_is_an_auth_failure_not_a_panic() {
    // A corrupted row must fail the login, not take the process down.
    let (hash, salt) = db::hash_password("x");
    assert!(!db::verify_password("x", "not-hex", &salt));
    assert!(!db::verify_password("x", &hash, "not-hex"));
    assert!(!db::verify_password("x", "", ""));
    assert!(!db::verify_password("x", "zz", &salt));
}

#[test]
fn an_empty_hash_or_salt_does_not_verify_anything() {
    let (_, salt) = db::hash_password("x");
    let (hash, _) = db::hash_password("x");
    assert!(!db::verify_password("x", "", &salt));
    assert!(!db::verify_password("x", &hash, ""));
    // Even the empty password must fail against an empty stored hash.
    assert!(!db::verify_password("", "", ""));
}

// ===================== language =====================

#[test]
fn the_two_supported_languages_round_trip() {
    assert_eq!(normalize_language(Some("zh-CN")), "zh-CN");
    assert_eq!(normalize_language(Some("en-US")), "en-US");
}

#[test]
fn an_unsupported_language_falls_back_to_chinese() {
    assert_eq!(normalize_language(Some("fr-FR")), "zh-CN");
    assert_eq!(normalize_language(Some("klingon")), "zh-CN");
    assert_eq!(normalize_language(Some("")), "zh-CN");
    assert_eq!(normalize_language(None), "zh-CN");
}

#[test]
fn the_language_check_is_an_exact_allowlist() {
    // Deliberately strict: the value round-trips into the settings table and
    // is later read by the router, so anything not literally one of the two
    // known locales falls back rather than being stored verbatim. A browser
    // sending a bare `en` therefore lands on Chinese — pinned here so that
    // loosening it (case folding, prefix matching) is a visible decision.
    assert_eq!(normalize_language(Some("EN-US")), "zh-CN");
    assert_eq!(normalize_language(Some("en")), "zh-CN");
    assert_eq!(normalize_language(Some("zh")), "zh-CN");
    assert_eq!(normalize_language(Some("  en-US  ")), "zh-CN");
}
