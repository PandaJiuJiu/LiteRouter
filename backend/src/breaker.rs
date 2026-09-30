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
    /// Any non-2xx (5xx, 4xx, 408, 429) or transport error.
    Failure,
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
}

impl BreakerState {
    fn open(base_delay: Duration, deadline: Instant) -> Self {
        Self {
            deadline,
            current_backoff: base_delay,
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
            Outcome::Failure => {
                let backoff = match guard.entry(key.to_owned()) {
                    Entry::Occupied(o) => o
                        .get()
                        .current_backoff
                        .saturating_mul(2)
                        .min(cfg.max_delay),
                    Entry::Vacant(v) => {
                        let deadline =
                            now.checked_add(cfg.base_delay).unwrap_or(now);
                        v.insert(BreakerState::open(cfg.base_delay, deadline));
                        cfg.base_delay
                    }
                };
                let entry = guard.get_mut(key).expect("just inserted");
                if backoff != entry.current_backoff {
                    entry.current_backoff = backoff;
                }
                entry.deadline =
                    now.checked_add(entry.current_backoff).unwrap_or(now);
            }
        }
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
        b.record(&key, Outcome::Failure).await;
        assert!(!b.allow(&key).await);
        assert_eq!(b.snapshot().await[0].state, "open");
    }

    #[tokio::test]
    async fn success_removes_entry_from_map() {
        // On Success the key is dropped entirely, not retained with a
        // closed marker. `snapshot()` should be empty and `allow()`
        // should return true again.
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "model");
        b.record(&key, Outcome::Failure).await;
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
        b.record(&key, Outcome::Failure).await;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(!b.allow(&key).await);
    }

    #[tokio::test]
    async fn expired_keys_lists_due_probes() {
        let b = Breaker::new(cfg());
        let key1 = breaker_key("ch1", "m1");
        let key2 = breaker_key("ch2", "m2");
        b.record(&key1, Outcome::Failure).await;
        b.record(&key2, Outcome::Failure).await;
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
            b.inner.read().await.get(key).unwrap().current_backoff.as_secs()
        }

        b.record(&key, Outcome::Failure).await;
        assert_eq!(backoff(&b, &key).await, 1);

        b.record(&key, Outcome::Failure).await;
        assert_eq!(backoff(&b, &key).await, 2);

        b.record(&key, Outcome::Failure).await;
        assert_eq!(backoff(&b, &key).await, 4);

        b.record(&key, Outcome::Failure).await;
        assert_eq!(backoff(&b, &key).await, 5);
        assert_eq!(b.snapshot().await[0].state, "open");

        // Cap holds for additional failures
        b.record(&key, Outcome::Failure).await;
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
        b.record(&key, Outcome::Failure).await;
        b.record(&key, Outcome::Failure).await;
        b.record(&key, Outcome::Failure).await;
        assert_eq!(backoff(&b, &key).await, Some(4));

        // Probe succeeds → entry removed entirely
        b.record(&key, Outcome::Success).await;
        assert_eq!(backoff(&b, &key).await, None);

        // Next failure should use base_delay, not 4s
        b.record(&key, Outcome::Failure).await;
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
            b.record(&key, Outcome::Failure).await;
        }
        // No state should be tracked at all when disabled.
        assert!(b.snapshot().await.is_empty());
    }

    #[tokio::test]
    async fn reset_clears_everything() {
        let b = Breaker::new(cfg());
        let k1 = breaker_key("ch1", "m");
        let k2 = breaker_key("ch2", "m");
        b.record(&k1, Outcome::Failure).await;
        b.record(&k2, Outcome::Failure).await;
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
        b.record(&k1, Outcome::Failure).await;
        b.record(&k2, Outcome::Failure).await;
        b.reset_key(&k1).await;
        let snap = b.snapshot().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].channel, "ch2");
    }
}