//! Background circuit-breaker probe task.
//!
//! Once a `(channel, model)` is tripped, recovery is the responsibility of
//! this task — user requests do NOT double as probes. Every `probe_interval`
//! seconds the task:
//!
//! 1. Asks the breaker for keys whose cooldown has elapsed.
//! 2. For each, sends a synthetic `max_tokens=1` ping to the channel.
//! 3. Records the outcome back into the breaker (Success closes it,
//!    Failure re-opens it with doubled back-off capped at `max_delay`).
//!
//! Probes use the channel's own API key directly (no user token is
//! involved) and the channel's protocol — we read the channel row from the
//! DB each tick so config edits are picked up without a restart.
//!
//! ## Wildcard channels
//!
//! Channels with `models = "*"` advertise support for any model without
//! enumerating them. To probe such a channel we fetch the live model list
//! from `GET /v1/models` and cache it for `model_cache_ttl`. Configured
//! non-wildcard `models` lists are used verbatim with no extra round-trip.

use crate::breaker::{self, Outcome, TransitionKind};
use crate::breaker_history::{record_breaker_event, BreakerEventKind, BreakerEventRow};
use crate::proxy;
use crate::state::AppState;
use serde_json::json;
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const MODEL_CACHE_TTL: Duration = Duration::from_secs(300);

/// One channel's view as far as the probe task is concerned: enough to
/// build and authenticate the synthetic request. `base_url_anthropic` may
/// be empty when the channel only speaks one protocol.
#[derive(Clone)]
struct ProbeChannel {
    name: String,
    api_key: String,
    base_url: String,
    base_url_anthropic: String,
    /// Raw `models` field from the DB (comma-separated, may contain `*`).
    models_csv: String,
}

/// Cached model list for a single channel. `models_csv` is a verbatim copy
/// of what the channel is configured for; the cached `models` is what we
/// actually probe (resolved from `*` if needed).
#[derive(Clone)]
struct CachedModels {
    models: Vec<String>,
    fetched_at: Instant,
}

/// Spawn the probe loop and return immediately. The task runs forever in
/// the background; the breaker can be disabled at runtime via the config
/// endpoint, in which case the task simply stops sending probes but still
/// ticks so the config change is observed.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let cache: Arc<RwLock<HashMap<String, CachedModels>>> =
            Arc::new(RwLock::new(HashMap::new()));
        // Read the interval once per tick so admin edits take effect without
        // a restart. First tick is delayed one full interval to avoid
        // spamming probes the moment the gateway boots.
        let mut interval = current_probe_interval(&state).await;
        loop {
            tokio::time::sleep(interval).await;
            interval = current_probe_interval(&state).await;
            if let Err(e) = sweep_once(&state, &cache).await {
                eprintln!("breaker probe sweep failed: {e}");
            }
        }
    });
}

async fn current_probe_interval(state: &AppState) -> Duration {
    state.breaker.config_snapshot().await.probe_interval
}

/// One full sweep: probe each due key and record its outcome. Errors
/// against the breaker are logged at info (this is normal recovery noise);
/// errors in setup (DB query, etc.) bubble out so the caller can log them.
async fn sweep_once(
    state: &AppState,
    cache: &Arc<RwLock<HashMap<String, CachedModels>>>,
) -> Result<ProbeReport, String> {
    let now = Instant::now();
    let due = state.breaker.expired_keys(now).await;
    sweep(state, cache, due).await
}

/// What a sweep actually did, so the manual endpoint can report it back
/// instead of the admin having to eyeball the panel to find out.
#[derive(Default, Debug, Clone, Copy)]
pub struct ProbeReport {
    /// Keys that were pinged.
    pub probed: usize,
    /// Of those, how many came back healthy and closed the breaker.
    pub recovered: usize,
}

/// Probe exactly the given keys — the shared body of the background ticker
/// and the admin's manual "probe now" button. Splitting it out is what keeps
/// the two paths from drifting apart.
async fn sweep(
    state: &AppState,
    cache: &Arc<RwLock<HashMap<String, CachedModels>>>,
    keys: Vec<(String, String)>,
) -> Result<ProbeReport, String> {
    let mut report = ProbeReport::default();
    if keys.is_empty() {
        return Ok(report);
    }
    // Group keys by channel so we only hit the DB once per channel.
    let mut by_channel: HashMap<String, Vec<String>> = HashMap::new();
    for (channel_name, model) in keys {
        by_channel.entry(channel_name).or_default().push(model);
    }
    for (channel_name, models) in by_channel {
        let Some(ch) = load_probe_channel(state, &channel_name)
            .await
            .map_err(|e| format!("load channel {channel_name}: {e}"))?
        else {
            // Channel was deleted or disabled — clear the breaker entry
            // so it stops appearing in the snapshot and consuming ticks.
            for m in &models {
                state
                    .breaker
                    .reset_key(&breaker::breaker_key(&channel_name, m))
                    .await;
                // Append a `reset_key` row so the history reflects *something*
                // removed the entry; the user's view shouldn't go dark.
                record_breaker_event(
                    &state.pool,
                    BreakerEventRow {
                        channel_name: channel_name.clone(),
                        target_model: m.clone(),
                        event: BreakerEventKind::ResetKey,
                        reason: "channel deleted or disabled".to_string(),
                        backoff_secs: 0,
                    },
                );
            }
            continue;
        };
        let probe_targets = resolve_models(state, cache, &ch).await;
        for m in &models {
            // The key may have been added to expired_keys() under a model
            // name that the channel no longer claims; skip rather than
            // ping a model the channel doesn't know.
            if !probe_targets.iter().any(|x| x == m) {
                continue;
            }
            let result = probe_one(state, &ch, m).await;
            let success = matches!(result, ProbeResult::Success);
            let transition = state
                .breaker
                .record(
                    &breaker::breaker_key(&channel_name, m),
                    if success {
                        Outcome::Success
                    } else {
                        Outcome::Failure(probe_reason(&result))
                    },
                )
                .await;
            // Append the matching history row. None = breaker disabled,
            // record() was a no-op — nothing to log.
            let event_kind = match transition.kind {
                TransitionKind::Inserted => Some(BreakerEventKind::Tripped),
                TransitionKind::Updated => Some(BreakerEventKind::ReTripped),
                TransitionKind::Removed => Some(BreakerEventKind::Recovered),
                TransitionKind::None => None,
            };
            if let Some(event) = event_kind {
                record_breaker_event(
                    &state.pool,
                    BreakerEventRow {
                        channel_name: channel_name.clone(),
                        target_model: m.clone(),
                        event,
                        reason: transition.reason,
                        backoff_secs: transition.backoff_secs,
                    },
                );
            }
            log_probe(&channel_name, m, &result);
            report.probed += 1;
            if success {
                report.recovered += 1;
            }
        }
    }
    Ok(report)
}

/// Admin-triggered sweep: probe every currently-tracked key right now,
/// ignoring the cooldown that the background ticker respects.
///
/// The point is an admin who has just fixed an upstream (rotated a key,
/// raised a rate limit) and does not want to wait out a 5-minute backoff
/// to find out. Probing a key that is still genuinely broken just records
/// another failure and doubles its backoff — the same thing the ticker
/// would have done a moment later, just sooner.
pub async fn probe_now(state: &AppState) -> Result<ProbeReport, String> {
    // A throwaway cache: a manual sweep shouldn't reuse the ticker's cached
    // model lists, and it must not be able to poison them either. The cache
    // only saves a `/v1/models` round-trip for wildcard channels.
    let cache = Arc::new(RwLock::new(HashMap::new()));
    let keys = state.breaker.tracked_keys().await;
    sweep(state, &cache, keys).await
}

/// Pick the right base URL for the channel: OpenAI style first, fall back
/// to Anthropic. We probe once per channel regardless of how many models
/// are listed — the protocol doesn't change between models.
async fn probe_one(state: &AppState, ch: &ProbeChannel, model: &str) -> ProbeResult {
    let (url, headers) = if !ch.base_url_anthropic.is_empty() {
        // Two-protocol channel: prefer OpenAI for probing (cheaper for most
        // providers) but fall back to Anthropic if OpenAI URL is missing.
        if !ch.base_url.is_empty() {
            let url = format!("{}/chat/completions", ch.base_url.trim_end_matches('/'));
            let h = vec![(
                "Authorization".to_string(),
                format!("Bearer {}", ch.api_key),
            )];
            (url, h)
        } else {
            let url = format!(
                "{}/v1/messages",
                ch.base_url_anthropic.trim_end_matches('/')
            );
            let h = vec![
                ("x-api-key".to_string(), ch.api_key.clone()),
                ("anthropic-version".to_string(), "2023-06-01".to_string()),
            ];
            (url, h)
        }
    } else if !ch.base_url.is_empty() {
        let url = format!("{}/chat/completions", ch.base_url.trim_end_matches('/'));
        let h = vec![(
            "Authorization".to_string(),
            format!("Bearer {}", ch.api_key),
        )];
        (url, h)
    } else {
        return ProbeResult::Error("channel has no base_url".into());
    };

    let body = json!({
        "model": model,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 1,
    });

    let mut req = state.http.post(&url).timeout(PROBE_TIMEOUT);
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = match req.json(&body).send().await {
        Ok(r) => r,
        Err(e) => return ProbeResult::Transport(proxy::describe_transport_error(&e)),
    };
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        ProbeResult::Success
    } else {
        ProbeResult::Http {
            status,
            detail: proxy::read_error_detail(resp).await,
        }
    }
}

#[derive(Debug)]
enum ProbeResult {
    Success,
    Http {
        status: u16,
        /// Upstream's own wording, same as the relay path. `None` when the
        /// body was empty or unreadable.
        detail: Option<String>,
    },
    Transport(String),
    Error(String),
}

/// One-line cause for the breaker panel. Mirrors the relay-side strings so
/// the two look alike in the snapshot: `HTTP 429: <detail>` /
/// `transport: <kind>`. The transport string arrives already formatted by
/// `describe_transport_error`, so it passes through untouched.
fn probe_reason(r: &ProbeResult) -> String {
    match r {
        ProbeResult::Success => String::new(),
        ProbeResult::Http { status, detail } => match detail {
            Some(d) => format!("HTTP {status}: {d}"),
            None => format!("HTTP {status}"),
        },
        ProbeResult::Transport(e) => e.clone(),
        ProbeResult::Error(e) => e.clone(),
    }
}

fn log_probe(channel: &str, model: &str, result: &ProbeResult) {
    // One info line per probe. Sample:
    //   breaker probe channel=openrouter model=gpt-4 result=success latency=...ms
    match result {
        ProbeResult::Success => {
            println!("breaker probe channel={channel} model={model} result=success")
        }
        ProbeResult::Http { status, .. } => {
            println!("breaker probe channel={channel} model={model} result=failure status={status}")
        }
        // Already carries its own `transport: ` prefix from
        // `describe_transport_error` — don't add a second one.
        ProbeResult::Transport(e) => {
            println!("breaker probe channel={channel} model={model} result=failure error={e}")
        }
        ProbeResult::Error(e) => {
            println!("breaker probe channel={channel} model={model} result=failure error={e}")
        }
    }
}

async fn load_probe_channel(
    state: &AppState,
    name: &str,
) -> Result<Option<ProbeChannel>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT name, api_key, base_url, base_url_anthropic, models, enabled FROM channels WHERE name = ?",
    )
    .bind(name)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let enabled: i64 = row.get("enabled");
    if enabled != 1 {
        return Ok(None);
    }
    Ok(Some(ProbeChannel {
        name: row.get("name"),
        api_key: row.get("api_key"),
        base_url: row.get("base_url"),
        base_url_anthropic: row.get("base_url_anthropic"),
        models_csv: row.get("models"),
    }))
}

/// Resolve the list of models to probe for `ch`. Uses the configured list
/// when it's a non-wildcard CSV; fetches from upstream (`GET /v1/models`)
/// otherwise, refreshing if the cache is older than `MODEL_CACHE_TTL`.
async fn resolve_models(
    state: &AppState,
    cache: &Arc<RwLock<HashMap<String, CachedModels>>>,
    ch: &ProbeChannel,
) -> Vec<String> {
    let explicit: Vec<String> = ch
        .models_csv
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "*")
        .collect();
    if !explicit.is_empty() {
        return explicit;
    }
    // Wildcard or empty: consult our cache, refresh if stale.
    {
        let guard = cache.read().await;
        if let Some(cached) = guard.get(&ch.name) {
            if cached.fetched_at.elapsed() < MODEL_CACHE_TTL {
                return cached.models.clone();
            }
        }
    }
    // Cache miss / stale — fetch.
    let fetched = fetch_models_from_upstream(state, ch).await;
    let mut guard = cache.write().await;
    let models = fetched.unwrap_or_default();
    guard.insert(
        ch.name.clone(),
        CachedModels {
            models: models.clone(),
            fetched_at: Instant::now(),
        },
    );
    models
}

/// Fetch the live model list from the channel via `GET /v1/models`. Returns
/// an empty Vec on any failure — the probe task prefers "skip" over
/// "abort" so a flaky `/models` endpoint doesn't break recovery.
async fn fetch_models_from_upstream(state: &AppState, ch: &ProbeChannel) -> Option<Vec<String>> {
    let url = if !ch.base_url.is_empty() {
        format!("{}/models", ch.base_url.trim_end_matches('/'))
    } else if !ch.base_url_anthropic.is_empty() {
        format!("{}/models", ch.base_url_anthropic.trim_end_matches('/'))
    } else {
        return None;
    };
    let mut req = state.http.get(&url).timeout(PROBE_TIMEOUT);
    if url.contains("/v1/messages") || url.starts_with(&ch.base_url_anthropic) {
        req = req
            .header("x-api-key", &ch.api_key)
            .header("anthropic-version", "2023-06-01");
    } else {
        req = req.bearer_auth(&ch.api_key);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    Some(
        body.get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default(),
    )
}
