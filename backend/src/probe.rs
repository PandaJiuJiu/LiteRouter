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
