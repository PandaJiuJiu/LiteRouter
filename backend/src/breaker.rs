//! Per-`(channel, model)` circuit breaker. In-memory only — process restart
//! clears every key.
//!
//! ## State machine
//!
//! ```text
//!   (absent) ──record(failure)──> Open   (deadline = now + base_delay)
//!      ▲                                    │
//!      │                       now >= deadline
//!      │                       (background task probes)
//!      │                                    │
//!      │                       record(failure) doubles backoff
//!      │                       (capped at max_delay)
//!      │                                    │
//!      └────────record(success)─────────────┘  (entry removed)
//! ```
//!
//! Two writers feed `record()`: the relay (after a user-driven upstream
//! attempt) and the probe task (after a synthetic recovery probe). The
//! backoff grows geometrically — first trip uses `base_delay`, every
//! subsequent probe failure doubles the wait, capped at `max_delay`.
//! On `Success` the entry is removed from the map entirely; the next
//! failure, if it happens, starts a fresh ladder at `base_delay`.
//!
//! ## Recovery
//!
//! Recovery is **never** triggered by user requests. When the deadline
//! elapses the breaker stays Open; only the background probe task, which
//! ticks every `probe_interval`, observes the deadline and sends a
//! synthetic probe. This keeps recovery independent of user traffic —
//! a healthy upstream recovers at the next probe tick after its cooldown,
//! not whenever someone happens to request the model again.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use axum::extract::State as AxumState;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::json;

use crate::auth::require_admin;
use crate::state::AppState;

/// Tunables. Defaults are loaded from the `settings` table at startup.
#[derive(Clone, Debug)]
pub struct BreakerConfig {
    pub enabled: bool,
    /// Backoff applied on the first failure (or after a success resets the
    /// ladder). Each subsequent failure *doubles* the backoff, capped at
    /// `max_delay` — geometric growth.
    pub base_delay: Duration,
    /// Cap on the per-key backoff. With base_delay=30s and max_delay=600s
    /// the ladder is 30 → 60 → 120 → 240 → 480 → 600, then it plateaus at
    /// 600s until either the upstream recovers or the admin resets.
    pub max_delay: Duration,
    /// Background probe task interval. The task ticks at this rate, sending
    /// a synthetic probe for every Open key whose `deadline <= now`.
    pub probe_interval: Duration,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_delay: Duration::from_secs(30),
            max_delay: Duration::from_secs(600),
            probe_interval: Duration::from_secs(30),
        }
    }
}

/// Outcome of a single upstream attempt, fed into `record()`.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// 2xx — counts as success.
    Success,
    /// Any non-2xx (5xx, 4xx, 408, 429) or transport error, plus a short
    /// human-readable cause — `"HTTP 429"`, `"transport: connect timeout"`,
    /// or a probe-side message. Surfaced in the admin snapshot so an
    /// operator can tell an outage from a bad key without reading logs.
    Failure(String),
}

/// Upstream errors are unbounded strings — a provider's error `message` can
/// be a paragraph, and `extract_error_msg`'s own raw fallback runs to 160
/// chars before this cap applies. The breaker panel shows the reason in a
/// tooltip, so clip rather than carry an arbitrary blob through the
/// snapshot.
///
/// Head-preserving is correct here because both producers now put the
/// load-bearing part first: `HTTP 429: <msg>` and `transport: <kind>` are
/// short prefixes, so a long `<msg>` is the only thing that ever gets cut.
const REASON_MAX: usize = 200;

fn clip_reason(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= REASON_MAX {
        return s.to_string();
    }
    let head: String = s.chars().take(REASON_MAX).collect();
    format!("{head}…")
}

/// JSON-friendly view of one key's state for the admin panel. Only
/// currently-Open keys appear here — Closed keys are dropped from the
/// breaker entirely (no "ever-touched" history is retained).
#[derive(Clone, Debug, Serialize)]
pub struct BreakerSnapshotRow {
    pub key: String,
    pub channel: String,
    pub target_model: String,
    pub state: String, // always "open" today — closed keys aren't tracked
    /// Why the key was last tripped, e.g. `"HTTP 429"` or
    /// `"transport: connect timeout"`. The *latest* failure wins — a probe
    /// that fails again overwrites the original trigger, which is usually
    /// the more useful signal since it reflects the current condition.
    pub reason: String,
    /// Seconds until the next probe is due.
    pub cooldown_remaining_secs: u64,
}

pub fn breaker_key(channel: &str, target_model: &str) -> String {
    format!("{channel}|{target_model}")
}

struct BreakerState {
    /// Hard deadline — the breaker must not let this key through until
    /// `Instant::now() >= deadline`. The background probe task checks
    /// this against `now` on every tick.
    deadline: Instant,
    /// Backoff currently in effect. Doubles on each consecutive failure,
    /// capped at `max_delay`. A `Success` removes the entry from the map,
    /// so this field is only meaningful for keys that are currently Open.
    current_backoff: Duration,
    /// Cause recorded by the most recent `Outcome::Failure`.
    reason: String,
}

impl BreakerState {
    fn open(base_delay: Duration, deadline: Instant, reason: String) -> Self {
        Self {
            deadline,
            current_backoff: base_delay,
            reason,
        }
    }
}

pub struct Breaker {
    cfg: Arc<RwLock<BreakerConfig>>,
    inner: Arc<RwLock<HashMap<String, BreakerState>>>,
}

impl Breaker {
    pub fn new(cfg: BreakerConfig) -> Self {
        Self {
            cfg: Arc::new(RwLock::new(cfg)),
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn config_snapshot(&self) -> BreakerConfig {
        self.cfg.read().await.clone()
    }

    /// Hot-swap the config (admin PUT handler). Existing OPEN timers keep
    /// their original deadline; only newly-recorded outcomes use the new
    /// timings.
    pub async fn replace_config(&self, cfg: BreakerConfig) {
        *self.cfg.write().await = cfg;
    }

    /// Decide whether a user-driven request to `key` should proceed.
    /// **Any tracked key is rejected** — Closed keys are absent from the
    /// map, Open keys have a future deadline. The background probe task
    /// is the only path that can re-enable a key. This keeps recovery
    /// independent of user traffic.
    pub async fn allow(&self, key: &str) -> bool {
        let cfg = self.cfg.read().await.clone();
        if !cfg.enabled {
            return true;
        }
        let guard = self.inner.read().await;
        !guard.contains_key(key)
    }

    /// Feed one outcome back into the breaker. Called from both the relay
    /// (user-driven attempts) and the probe task (synthetic probes).
    pub async fn record(&self, key: &str, outcome: Outcome) {
        let cfg = self.cfg.read().await.clone();
        if !cfg.enabled {
            return;
        }
        let now = Instant::now();
        let mut guard = self.inner.write().await;
        match outcome {
            Outcome::Success => {
                // Drop the entry entirely — the next failure, if any,
                // starts a fresh ladder at `base_delay`.
                guard.remove(key);
            }
            Outcome::Failure(reason) => {
                let reason = clip_reason(&reason);
                let backoff = match guard.entry(key.to_owned()) {
                    Entry::Occupied(o) => {
                        o.get().current_backoff.saturating_mul(2).min(cfg.max_delay)
                    }
                    Entry::Vacant(v) => {
                        let deadline = now.checked_add(cfg.base_delay).unwrap_or(now);
                        v.insert(BreakerState::open(cfg.base_delay, deadline, reason.clone()));
                        cfg.base_delay
                    }
                };
                let entry = guard.get_mut(key).expect("just inserted");
                if backoff != entry.current_backoff {
                    entry.current_backoff = backoff;
                }
                entry.deadline = now.checked_add(entry.current_backoff).unwrap_or(now);
                // Latest failure wins — see `BreakerSnapshotRow::reason`.
                entry.reason = reason;
            }
        }
    }

    /// Every tracked key, regardless of cooldown. This is what a *manual*
    /// probe sweep uses — an admin who just fixed an upstream wants to test
    /// recovery now, not after the remaining backoff elapses.
    ///
    /// The background ticker must use [`Self::expired_keys`] instead: probing
    /// ahead of the deadline would defeat the backoff entirely.
    pub async fn tracked_keys(&self) -> Vec<(String, String)> {
        let guard = self.inner.read().await;
        guard.keys().map(|k| split_key_owned(k)).collect()
    }

    /// Keys that the probe task should attempt on the next tick: every
    /// tracked key whose `deadline <= now`. We don't proactively probe
    /// healthy models because that would burn quota for no diagnostic
    /// value — only keys we already know are Open get probed.
    pub async fn expired_keys(&self, now: Instant) -> Vec<(String, String)> {
        let guard = self.inner.read().await;
        guard
            .iter()
            .filter_map(|(k, e)| {
                if e.deadline <= now {
                    Some(split_key_owned(k))
                } else {
                    None
                }
            })
            .collect()
    }

    pub async fn snapshot(&self) -> Vec<BreakerSnapshotRow> {
        let guard = self.inner.read().await;
        let now = Instant::now();
        let mut out: Vec<BreakerSnapshotRow> = Vec::new();
        for (key, entry) in guard.iter() {
            let remain = entry.deadline.saturating_duration_since(now).as_secs();
            let (channel, target_model) = split_key_owned(key);
            out.push(BreakerSnapshotRow {
                key: key.clone(),
                channel,
                target_model,
                state: "open".to_string(),
                reason: entry.reason.clone(),
                cooldown_remaining_secs: remain,
            });
        }
        out
    }

    pub async fn reset(&self) {
        let mut guard = self.inner.write().await;
        guard.clear();
    }

    /// Drop a single key. Used by the probe task when its channel row
    /// disappears (deleted or disabled) — the key would otherwise sit in
    /// `Open` forever, showing up in the snapshot and consuming ticks.
    pub async fn reset_key(&self, key: &str) {
        let mut guard = self.inner.write().await;
        guard.remove(key);
    }
}

fn split_key_owned(key: &str) -> (String, String) {
    match key.split_once('|') {
        Some((c, m)) => (c.to_string(), m.to_string()),
        None => (String::new(), key.to_string()),
    }
}

// ---- HTTP handlers (admin) ------------------------------------------------

/// GET /api/breaker/snapshot
pub async fn http_snapshot(
    AxumState(state): AxumState<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    if require_admin(&state, &headers).is_err() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let snap = state.breaker.snapshot().await;
    Json(json!({ "snapshot": snap })).into_response()
}

/// POST /api/breaker/reset — clear every key's state. Useful after a known
/// upstream incident to bring everything back to CLOSED immediately.
pub async fn http_reset(
    AxumState(state): AxumState<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    if require_admin(&state, &headers).is_err() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    state.breaker.reset().await;
    Json(json!({ "ok": true })).into_response()
}

/// POST /api/breaker/probe-now — probe every tracked key immediately,
/// ignoring the remaining cooldown, and report what came back.
///
/// The background ticker only probes a key once its backoff has elapsed, so
/// an admin who has just fixed an upstream would otherwise wait out the
/// delay to find out whether the fix worked.
pub async fn http_probe_now(
    AxumState(state): AxumState<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    if require_admin(&state, &headers).is_err() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let report = match crate::breaker_probe::probe_now(&state).await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("探测失败: {e}") })),
            )
                .into_response()
        }
    };
    // Return the fresh snapshot too, so the panel can re-render without a
    // second round-trip.
    let snap = state.breaker.snapshot().await;
    Json(json!({
        "ok": true,
        "probed": report.probed,
        "recovered": report.recovered,
        "snapshot": snap,
    }))
    .into_response()
}

// ---- tests ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> BreakerConfig {
        BreakerConfig {
            enabled: true,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(5),
            probe_interval: Duration::from_millis(500),
        }
    }

    #[tokio::test]
    async fn allow_through_when_unseen() {
        let b = Breaker::new(cfg());
        assert!(b.allow(&breaker_key("ch", "model")).await);
    }

    #[tokio::test]
    async fn single_failure_trips_breaker() {
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "model");
        assert!(b.allow(&key).await);
        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        assert!(!b.allow(&key).await);
        assert_eq!(b.snapshot().await[0].state, "open");
        assert_eq!(b.snapshot().await[0].reason, "HTTP 500");
    }

    #[tokio::test]
    async fn the_latest_failure_overwrites_the_recorded_reason() {
        // A probe that fails again tells you more than the original trip —
        // it's the current condition — so the newer cause replaces the old
        // one rather than being discarded.
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "model");
        b.record(&key, Outcome::Failure("HTTP 429".into())).await;
        b.record(&key, Outcome::Failure("transport: timeout".into()))
            .await;
        assert_eq!(b.snapshot().await[0].reason, "transport: timeout");
    }

    #[tokio::test]
    async fn a_very_long_reason_is_clipped_for_the_snapshot() {
        // reqwest's `Display` chains every cause; the panel only shows the
        // string in a tooltip.
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "model");
        let long = "transport: ".to_string() + &"x".repeat(500);
        b.record(&key, Outcome::Failure(long)).await;
        let reason = b.snapshot().await[0].reason.clone();
        assert!(reason.ends_with('…'), "got {reason:?}");
        assert_eq!(reason.chars().count(), REASON_MAX + 1);
    }

    #[tokio::test]
    async fn success_removes_entry_from_map() {
        // On Success the key is dropped entirely, not retained with a
        // closed marker. `snapshot()` should be empty and `allow()`
        // should return true again.
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "model");
        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        assert!(!b.allow(&key).await);
        assert_eq!(b.snapshot().await.len(), 1);

        b.record(&key, Outcome::Success).await;
        assert!(b.allow(&key).await);
        assert!(b.snapshot().await.is_empty());
    }

    #[tokio::test]
    async fn open_key_is_not_re_enabled_by_user_request_after_cooldown() {
        // The whole point: a user request arriving after cooldown must NOT
        // itself act as a probe. Only the background task does probing.
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "model");
        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(!b.allow(&key).await);
    }

    #[tokio::test]
    async fn expired_keys_lists_due_probes() {
        let b = Breaker::new(cfg());
        let key1 = breaker_key("ch1", "m1");
        let key2 = breaker_key("ch2", "m2");
        b.record(&key1, Outcome::Failure("HTTP 500".into())).await;
        b.record(&key2, Outcome::Failure("HTTP 500".into())).await;
        // Right after trip: none are due (cooldown = 1s)
        let now = Instant::now();
        assert!(b.expired_keys(now).await.is_empty());
        // After base_delay has elapsed: both are due
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let now = Instant::now();
        let due = b.expired_keys(now).await;
        assert_eq!(due.len(), 2);
    }

    #[tokio::test]
    async fn consecutive_failures_double_backoff() {
        // First failure: backoff = base_delay (1s)
        // 2nd failure (still Open from 1st): backoff = 2s
        // 3rd failure: backoff = 4s
        // 4th failure: backoff = 5s (= max_delay, doubled 8 → capped)
        // 5th failure: still 5s (cap holds)
        //
        // No sleeps between failures — the 1st record inserts a fresh
        // entry that uses `base_delay`; every subsequent record enters
        // the "Occupied" branch and doubles the backoff.
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "model");

        async fn backoff(b: &Breaker, key: &str) -> u64 {
            b.inner
                .read()
                .await
                .get(key)
                .unwrap()
                .current_backoff
                .as_secs()
        }

        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        assert_eq!(backoff(&b, &key).await, 1);

        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        assert_eq!(backoff(&b, &key).await, 2);

        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        assert_eq!(backoff(&b, &key).await, 4);

        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        assert_eq!(backoff(&b, &key).await, 5);
        assert_eq!(b.snapshot().await[0].state, "open");

        // Cap holds for additional failures
        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        assert_eq!(backoff(&b, &key).await, 5);
    }

    #[tokio::test]
    async fn success_resets_backoff_ladder() {
        // After success the entry is removed; the next failure starts a
        // fresh ladder at base_delay, not at the previously doubled value.
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "model");

        async fn backoff(b: &Breaker, key: &str) -> Option<u64> {
            b.inner
                .read()
                .await
                .get(key)
                .map(|s| s.current_backoff.as_secs())
        }

        // Fail three times → backoff should have doubled to 4s
        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        assert_eq!(backoff(&b, &key).await, Some(4));

        // Probe succeeds → entry removed entirely
        b.record(&key, Outcome::Success).await;
        assert_eq!(backoff(&b, &key).await, None);

        // Next failure should use base_delay, not 4s
        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        assert_eq!(backoff(&b, &key).await, Some(1));
    }

    #[tokio::test]
    async fn disabled_breaker_always_allows() {
        let mut c = cfg();
        c.enabled = false;
        let b = Breaker::new(c);
        let key = breaker_key("ch", "model");
        for _ in 0..20 {
            assert!(b.allow(&key).await);
            b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        }
        // No state should be tracked at all when disabled.
        assert!(b.snapshot().await.is_empty());
    }

    #[tokio::test]
    async fn reset_clears_everything() {
        let b = Breaker::new(cfg());
        let k1 = breaker_key("ch1", "m");
        let k2 = breaker_key("ch2", "m");
        b.record(&k1, Outcome::Failure("HTTP 500".into())).await;
        b.record(&k2, Outcome::Failure("HTTP 500".into())).await;
        assert_eq!(b.snapshot().await.len(), 2);
        b.reset().await;
        assert!(b.snapshot().await.is_empty());
        assert!(b.allow(&k1).await);
        assert!(b.allow(&k2).await);
    }

    #[tokio::test]
    async fn reset_key_drops_single_entry() {
        let b = Breaker::new(cfg());
        let k1 = breaker_key("ch1", "m");
        let k2 = breaker_key("ch2", "m");
        b.record(&k1, Outcome::Failure("HTTP 500".into())).await;
        b.record(&k2, Outcome::Failure("HTTP 500".into())).await;
        b.reset_key(&k1).await;
        let snap = b.snapshot().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].channel, "ch2");
    }

    #[tokio::test]
    async fn a_key_is_scoped_to_both_channel_and_model() {
        // The same model failing on one channel says nothing about another
        // channel's health for that model.
        let b = Breaker::new(cfg());
        let a = breaker_key("ch1", "gpt-4o");
        let c = breaker_key("ch2", "gpt-4o");
        let d = breaker_key("ch1", "claude-x");
        b.record(&a, Outcome::Failure("HTTP 500".into())).await;
        assert!(!b.allow(&a).await);
        assert!(b.allow(&c).await);
        assert!(b.allow(&d).await);
    }

    #[tokio::test]
    async fn a_channel_or_model_containing_the_separator_still_round_trips() {
        // Keys are `channel|model` and `split_key_owned` splits on the first
        // `|`. A model name containing `|` must not make the snapshot report
        // the wrong channel.
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "weird|model|name");
        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        let snap = b.snapshot().await;
        assert_eq!(snap[0].channel, "ch");
        assert_eq!(snap[0].target_model, "weird|model|name");
    }

    #[tokio::test]
    async fn the_backoff_ladder_is_capped_at_max_delay() {
        // base 1s, max 5s: 1, 2, 4, 5, 5...
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "m");
        let mut seen = Vec::new();
        for _ in 0..6 {
            b.record(&key, Outcome::Failure("HTTP 500".into())).await;
            seen.push(
                b.inner
                    .read()
                    .await
                    .get(&key)
                    .unwrap()
                    .current_backoff
                    .as_secs(),
            );
        }
        assert_eq!(seen, vec![1, 2, 4, 5, 5, 5]);
    }

    #[tokio::test]
    async fn replacing_the_config_takes_effect_without_a_restart() {
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "m");
        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        assert!(!b.allow(&key).await);

        let mut next = cfg();
        next.base_delay = Duration::from_secs(30);
        b.replace_config(next).await;
        assert_eq!(
            b.config_snapshot().await.base_delay,
            Duration::from_secs(30)
        );
        // An already-open key keeps doubling from where its own ladder left
        // off — the new base applies to the *first* failure of a fresh entry,
        // not retroactively to a key that is already up the ladder.
        assert!(!b.allow(&key).await);
        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        assert_eq!(
            b.inner.read().await.get(&key).unwrap().current_backoff,
            Duration::from_secs(2)
        );

        // Once the key clears, the next trip starts from the new base_delay.
        b.record(&key, Outcome::Success).await;
        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        assert_eq!(
            b.inner.read().await.get(&key).unwrap().current_backoff,
            Duration::from_secs(30)
        );
    }

    #[tokio::test]
    async fn disabling_the_breaker_mid_flight_stops_short_circuiting() {
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "m");
        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        assert!(!b.allow(&key).await);

        let mut next = cfg();
        next.enabled = false;
        b.replace_config(next).await;
        assert!(
            b.allow(&key).await,
            "turning the breaker off must unblock traffic"
        );
    }

    #[tokio::test]
    async fn expired_keys_reports_each_key_as_its_own_pair() {
        let b = Breaker::new(cfg());
        b.record(
            &breaker_key("ch1", "m1"),
            Outcome::Failure("HTTP 500".into()),
        )
        .await;
        b.record(
            &breaker_key("ch2", "m2"),
            Outcome::Failure("HTTP 500".into()),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let mut due = b.expired_keys(Instant::now()).await;
        due.sort();
        assert_eq!(
            due,
            vec![("ch1".into(), "m1".into()), ("ch2".into(), "m2".into())]
        );
    }

    #[tokio::test]
    async fn a_key_reset_only_drops_the_named_channel_model_pair() {
        // The probe task calls this when a channel disappears; the same model
        // on a different channel must be unaffected.
        let b = Breaker::new(cfg());
        let a = breaker_key("ch1", "m");
        let c = breaker_key("ch2", "m");
        b.record(&a, Outcome::Failure("HTTP 500".into())).await;
        b.record(&c, Outcome::Failure("HTTP 500".into())).await;
        b.reset_key(&a).await;
        assert!(b.allow(&a).await);
        assert!(!b.allow(&c).await);
    }

    #[tokio::test]
    async fn a_successful_probe_does_not_allow_a_user_request_to_re_enable_the_key() {
        // Recovery is owned by the background probe task. If a user request
        // arriving after cooldown could re-enable the key on its own, a
        // persistently broken upstream would get hammered by exactly the
        // traffic that should be spared.
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "m");
        b.record(&key, Outcome::Failure("HTTP 500".into())).await;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(
            !b.allow(&key).await,
            "cooldown alone must not restore traffic"
        );
        b.record(&key, Outcome::Success).await; // the probe
        assert!(b.allow(&key).await);
    }

    #[tokio::test]
    async fn the_cooldown_counts_down_in_the_snapshot() {
        // The admin snapshot shows whole seconds remaining, so the config needs
        // a delay long enough that truncation doesn't floor the first reading
        // at zero.
        let mut c = cfg();
        c.base_delay = Duration::from_secs(30);
        let b = Breaker::new(c);
        b.record(&breaker_key("ch", "m"), Outcome::Failure("HTTP 500".into()))
            .await;
        let first = b.snapshot().await[0].cooldown_remaining_secs;
        // Truncated to whole seconds, so a few microseconds of elapsed time
        // can already have knocked it down by one.
        assert!(
            (29..=30).contains(&first),
            "unexpected initial cooldown: {first}"
        );
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let second = b.snapshot().await[0].cooldown_remaining_secs;
        assert!(
            second < first,
            "expected the countdown to advance: {first} -> {second}"
        );
    }
}
