//! Shared "is this channel actually alive?" probe.
//!
//! Two callers need the exact same synthetic request and the exact same
//! verdict on the answer:
//!
//! - [`crate::breaker_probe`] — the background recovery ticker, which pings
//!   a tripped `(channel, model)` to decide whether the breaker can close.
//! - [`crate::admin::test_model`] — the models page's per-model test button.
//!
//! They used to build the request independently, and drifted: the ticker
//! sent `content: "ping"` while the admin UI sent `content: "."`. Against a
//! provider that meters by prompt length (Volcengine Ark, as of 2026-10)
//! that difference is the whole verdict — `"."` came back 200 while
//! `"ping"` came back 429, so the models page showed a dead, quota-exhausted
//! account as three green checkmarks. One definition, one verdict.
//!
//! ## Why not `max_tokens: 1`
//!
//! The obvious minimal probe asks for a single token. Two provider classes
//! reject it outright, which is the same trap LiteLLM walked into twice
//! (issues #23836 and #26987):
//!
//! - OpenAI's GPT-5 family rejects `max_output_tokens < 16` outright.
//! - Reasoning models (GPT-5.5, DeepSeek, Grok reasoning, Claude thinking)
//!   spend the *entire* budget on reasoning tokens, so a 1-token cap yields
//!   zero visible output and the request fails with "could not finish the
//!   message" — on a model that is working perfectly.
//!
//! [`PROBE_MAX_TOKENS`] clears the provider minimum with room for a
//! reasoning preamble, and [`PROBE_PROMPT`] asks for a single word so a
//! success is still cheap. `temperature: 0` keeps the answer stable.
//!
//! The prompt is deliberately longer than one character: a meter that
//! exempts very short inputs would otherwise let an exhausted account
//! report healthy. It stays short enough to cost a fraction of a cent.

use serde_json::{json, Value};

/// What we ask the model to say. Short, but several characters long so a
/// length-metered provider still charges (and therefore still rate-limits)
/// the probe. See the module docs for why it isn't `"."`.
pub const PROBE_PROMPT: &str = "Reply with the single word: OK";

/// Output cap for the probe. Above the OpenAI minimum of 16 so GPT-5-class
/// models accept it, and large enough that a reasoning model still has
/// tokens left for visible output. See the module docs.
pub const PROBE_MAX_TOKENS: u32 = 16;

/// The request body both probe paths send. `stream` is left off: a
/// non-streaming request is the cheapest thing to ask for and keeps the
/// response small enough to read for an error message.
pub fn probe_body(model: &str) -> Value {
    json!({
        "model": model,
        "messages": [{"role": "user", "content": PROBE_PROMPT}],
        "max_tokens": PROBE_MAX_TOKENS,
        "temperature": 0,
    })
}

/// How the upstream answered.
#[derive(Debug)]
pub enum ProbeOutcome {
    /// 2xx — the channel and model are reachable.
    Success,
    /// A non-2xx response. `detail` is the upstream's own wording, already
    /// extracted and truncated by [`crate::proxy::read_error_detail`].
    Http { status: u16, detail: Option<String> },
    /// The request exceeded `timeout`.
    Timeout(std::time::Duration),
    /// DNS/connect/TLS/body failure — never reached a status code.
    Transport(String),
    /// We never sent anything: the channel is misconfigured (e.g. it has
    /// no base URL to probe). Not an upstream verdict, but it has to be
    /// reportable for the same reason `Transport` is.
    Misconfigured(String),
}

impl ProbeOutcome {
    pub fn is_success(&self) -> bool {
        matches!(self, ProbeOutcome::Success)
    }

    /// One-line cause for the breaker panel and the `test-model` response.
    /// The relay path formats its reasons the same way, so a channel that
    /// trips from user traffic and one that trips from a probe look alike
    /// in the history table.
    pub fn reason(&self) -> String {
        match self {
            ProbeOutcome::Success => String::new(),
            ProbeOutcome::Http { status, detail } => match detail {
                Some(d) => format!("HTTP {status}: {d}"),
                None => format!("HTTP {status}"),
            },
            ProbeOutcome::Timeout(d) => format!("请求超时（> {}s）", d.as_secs()),
            ProbeOutcome::Transport(e) => e.clone(),
            ProbeOutcome::Misconfigured(e) => e.clone(),
        }
    }
}

/// Send one probe and classify the answer.
///
/// `url` and `headers` are the caller's to build: the breaker probes one
/// protocol per channel, while the admin UI tests both and reports them
/// side by side, so the choice of endpoint is not ours to make here.
pub async fn send(
    client: &reqwest::Client,
    url: &str,
    headers: &[(&str, &str)],
    model: &str,
    timeout: std::time::Duration,
) -> ProbeOutcome {
    let mut req = client.post(url).timeout(timeout);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if let Some((name, value)) = synthetic_session_header(url, model) {
        req = req.header(name, value);
    }
    let resp = match req.json(&probe_body(model)).send().await {
        Ok(r) => r,
        Err(e) if e.is_timeout() => return ProbeOutcome::Timeout(timeout),
        Err(e) => return ProbeOutcome::Transport(crate::proxy::describe_transport_error(&e)),
    };
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        ProbeOutcome::Success
    } else {
        ProbeOutcome::Http {
            status,
            detail: crate::proxy::read_error_detail(resp).await,
        }
    }
}

/// The synthetic session header a probe carries when the target expects one.
///
/// Real traffic identifies each conversation the way the client asked for
/// it — the relay re-sends the client's own session header verbatim (see
/// `proxy::SESSION_PASSTHROUGH_HEADERS`). A probe is not a conversation: it
/// should never *invent* per-request IDs for a provider that meters
/// sessions, and it should never omit the identifier for a provider that
/// **gates on it**. OpenCode Go does the second thing: requests without
/// `x-opencode-session` are rejected with 400 `missing_session_id`, which
/// made a perfectly alive channel read as dead — the breaker backed off a
/// healthy provider and the test button showed 400s carrying no signal.
///
/// So on an OpenCode host the probe speaks as its own conversation: one
/// stable synthetic ID per (upstream, model), deterministic across
/// restarts. The value is ours by construction — recognisable, and not any
/// client's conversation.
///
/// Scoped to `opencode.ai` and not fired everywhere: vendors not in on the
/// convention ignore unknown headers today, but "probe-shaped extra header"
/// is not a wire shape other providers have agreed to meet us halfway on.
///
/// The hash is `DefaultHasher::new()`, whose keys are fixed — the same
/// (url, model) pair derives the same value across processes, so the
/// provider sees one long-lived conversation rather than a fresh one per
/// probe.
fn synthetic_session_header(url: &str, model: &str) -> Option<(&'static str, String)> {
    let host = reqwest::Url::parse(url).ok()?.host_str()?.to_owned();
    if host != "opencode.ai" && !host.ends_with(".opencode.ai") {
        return None;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hasher::write(&mut h, url.as_bytes());
    std::hash::Hasher::write(&mut h, model.as_bytes());
    Some((
        "x-opencode-session",
        format!("litrouter-probe-{:016x}", std::hash::Hasher::finish(&h)),
    ))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ---------- synthetic_session_header ----------

    #[test]
    fn an_opencode_target_gets_a_stable_probe_conversation() {
        let (name, first) = synthetic_session_header(
            "https://opencode.ai/inference/go/openai/v1/chat/completions",
            "beam-free",
        )
        .expect("an opencode.ai url must produce a session id");
        assert_eq!(name, "x-opencode-session");
        assert!(first.starts_with("litrouter-probe-"), "got {first:?}");

        // Deterministic, not random: the same (upstream, model) pair is the
        // same conversation for the provider across probe calls and process
        // restarts, which is what "one stable id per conversation" means.
        assert_eq!(
            synthetic_session_header(
                "https://opencode.ai/inference/go/openai/v1/chat/completions",
                "beam-free"
            ),
            Some((name, first.clone())),
            "the same probe changed its conversation"
        );
        // Different model = different conversation, so two models' probes
        // are never conflated into one session bucket upstream; so is a
        // different endpoint — each upstream gets its own.
        assert_ne!(
            synthetic_session_header(
                "https://opencode.ai/inference/go/openai/v1/chat/completions",
                "deepseek-v4.1-flash",
            )
            .unwrap(),
            (name, first.clone())
        );
        assert_ne!(
            synthetic_session_header(
                "https://api.opencode.ai/inference/go/openai/v1/chat/completions",
                "beam-free",
            )
            .unwrap(),
            (name, first)
        );
    }

    #[test]
    fn everyone_else_gets_no_session_header() {
        // The check must not pass merely because the host *contains* the
        // tokens — `notopencode.ai` is some unrelated vendor.
        for url in [
            "https://notopencode.ai/v1/chat/completions",
            "http://127.0.0.1:8080/chat/completions",
            "opencode.ai/v1/chat/completions",
        ] {
            assert!(
                synthetic_session_header(url, "beam-free").is_none(),
                "{url} must get no synthetic session"
            );
        }
    }

    #[tokio::test]
    async fn a_probe_to_another_vendor_carries_no_session_header() {
        // The negative half through the real send path: the header is
        // injected only where it gates, so an unknown vendor sees an
        // ordinary probe-shaped request. (The positive half can't go through
        // `send` in a test — there is no opencode.ai endpoint to hit on
        // localhost — it lives in the pure-function tests above.)
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let url = format!("{}/chat/completions", server.uri());

        let outcome = send(
            &reqwest::Client::new(),
            &url,
            &[("Authorization", "Bearer test")],
            "beam-free",
            Duration::from_secs(5),
        )
        .await;
        assert!(outcome.is_success(), "probe prerequisite: got {outcome:?}");

        let reqs = server.received_requests().await.unwrap();
        assert!(
            reqs[0].headers.get("x-opencode-session").is_none(),
            "invented a session header for a vendor that never asked for one"
        );
    }
}
