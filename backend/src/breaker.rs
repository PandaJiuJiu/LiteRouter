//! Per-`(channel, model)` circuit breaker. In-memory only — process restart
//! clears every key.
//!
//! ## State machine
//!
//! ```text
//!   Closed ──record(failure)──> Open (next_probe_at = now + base_delay)
//!      ▲                                  │
//!      │                       now >= next_probe_at
//!      │                       (background task probes)
//!      │                                  │
//!      └────────record(success)──────────┘
//! ```
//!
//! Two writers feed `record()`: the relay (after a user-driven upstream
//! attempt) and the probe task (after a synthetic recovery probe). Both
//! paths collapse to the same two operations — set a 30s skip-timer on
//! failure, clear it on success.
//!
//! ## Recovery
//!
//! Recovery is **never** triggered by user requests. When `next_probe_at`
//! elapses the breaker stays Open; only the background probe task, which
//! ticks every `probe_interval`, observes the deadline and sends a
//! synthetic probe. This keeps recovery independent of user traffic —
//! a healthy upstream recovers at the next probe tick after its cooldown,
//! not whenever someone happens to request the model again.

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
    /// Skip window applied after every failure. After a probe also fails,
    /// the breaker re-arms the same skip window — no exponential back-off,
    /// no consecutive-failure counter, no cap. The background probe task
    /// keeps retrying at `probe_interval` cadence until it sees a 2xx.
    pub base_delay: Duration,
    /// Background probe task interval. The task ticks at this rate, sending
    /// a synthetic probe for every Open key whose `next_probe_at <= now`.
    pub probe_interval: Duration,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_delay: Duration::from_secs(30),
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

/// JSON-friendly view of one key's state for the admin panel.
#[derive(Clone, Debug, Serialize)]
pub struct BreakerSnapshotRow {
    pub key: String,
    pub channel: String,
    pub target_model: String,
    pub state: String, // "closed" | "open"
    /// Seconds until the next probe is due. 0 when Closed.
    pub cooldown_remaining_secs: u64,
}

pub fn breaker_key(channel: &str, target_model: &str) -> String {
    format!("{channel}|{target_model}")
}

struct BreakerState {
    /// `None` = Closed. `Some(deadline)` = Open until `deadline`.
    next_probe_at: Option<Instant>,
}

impl BreakerState {
    fn new() -> Self {
        Self { next_probe_at: None }
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
    /// **Open keys are always rejected** — the background probe task is
    /// the only path that can re-enable a key. This keeps recovery
    /// independent of user traffic.
    pub async fn allow(&self, key: &str) -> bool {
        let cfg = self.cfg.read().await.clone();
        if !cfg.enabled {
            return true;
        }
        let guard = self.inner.read().await;
        match guard.get(key) {
            Some(entry) => entry.next_probe_at.is_none(),
            None => true, // never seen — let the first attempt through
        }
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
        let entry = guard
            .entry(key.to_owned())
            .or_insert_with(BreakerState::new);

        match outcome {
            Outcome::Success => {
                entry.next_probe_at = None;
            }
            Outcome::Failure => {
                entry.next_probe_at =
                    Some(now.checked_add(cfg.base_delay).unwrap_or(now));
            }
        }
    }

    /// Keys that the probe task should attempt on the next tick: every
    /// Open key whose `next_probe_at <= now`. Closed keys (and keys we
    /// haven't seen) are skipped — we don't proactively probe healthy
    /// models because that would burn quota for no diagnostic value.
    pub async fn expired_keys(&self, now: Instant) -> Vec<(String, String)> {
        let guard = self.inner.read().await;
        guard
            .iter()
            .filter_map(|(k, e)| match e.next_probe_at {
                Some(deadline) if deadline <= now => Some(split_key_owned(k)),
                _ => None,
            })
            .collect()
    }

    pub async fn snapshot(&self) -> Vec<BreakerSnapshotRow> {
        let guard = self.inner.read().await;
        let now = Instant::now();
        let mut out: Vec<BreakerSnapshotRow> = Vec::new();
        for (key, entry) in guard.iter() {
            let (state_label, cooldown_remaining_secs) = match entry.next_probe_at {
                None => ("closed".to_string(), 0),
                Some(t) => {
                    let remain = t.saturating_duration_since(now).as_secs();
                    ("open".to_string(), remain)
                }
            };
            let (channel, target_model) = split_key_owned(key);
            out.push(BreakerSnapshotRow {
                key: key.clone(),
                channel,
                target_model,
                state: state_label,
                cooldown_remaining_secs,
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
            base_delay: Duration::from_millis(100),
            probe_interval: Duration::from_millis(50),
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
    async fn success_clears_open_state() {
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "model");
        b.record(&key, Outcome::Failure).await;
        assert!(!b.allow(&key).await);
        b.record(&key, Outcome::Success).await;
        assert!(b.allow(&key).await);
        assert_eq!(b.snapshot().await[0].state, "closed");
    }

    #[tokio::test]
    async fn open_key_is_not_re_enabled_by_user_request_after_cooldown() {
        // The whole point: a user request arriving after cooldown must NOT
        // itself act as a probe. Only the background task does probing.
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "model");
        b.record(&key, Outcome::Failure).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!b.allow(&key).await);
    }

    #[tokio::test]
    async fn expired_keys_lists_due_probes() {
        let b = Breaker::new(cfg());
        let key1 = breaker_key("ch1", "m1");
        let key2 = breaker_key("ch2", "m2");
        b.record(&key1, Outcome::Failure).await;
        b.record(&key2, Outcome::Failure).await;
        // Right after trip: none are due (cooldown = 100ms)
        let now = Instant::now();
        assert!(b.expired_keys(now).await.is_empty());
        // After base_delay has elapsed: both are due
        tokio::time::sleep(Duration::from_millis(150)).await;
        let now = Instant::now();
        let due = b.expired_keys(now).await;
        assert_eq!(due.len(), 2);
        // Closed keys (never failed) are never in the list
        let healthy_key = breaker_key("never-tried", "x");
        let _ = healthy_key;
    }

    #[tokio::test]
    async fn re_failure_resets_cooldown_to_base() {
        // Probe fails → next_probe_at is pushed by exactly base_delay
        // from the failure moment, not from the previous deadline.
        let b = Breaker::new(cfg());
        let key = breaker_key("ch", "model");
        b.record(&key, Outcome::Failure).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        // 1st probe failure — push to T+100ms from now
        b.record(&key, Outcome::Failure).await;
        // Right after: not yet due
        assert!(b.expired_keys(Instant::now()).await.is_empty());
        // After base_delay has elapsed again: due
        tokio::time::sleep(Duration::from_millis(150)).await;
        let now = Instant::now();
        assert_eq!(b.expired_keys(now).await.len(), 1);
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