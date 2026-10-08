// =========================== streaming ===========================

use super::responses::{finish_anthropic_to_openai, finish_openai_to_anthropic};
use super::{json, Value};
use crate::db;

/// Full token breakdown for one request. `prompt`/`completion`/`total` are
/// the headline numbers; the rest are the sub-counters that explain them.
/// Field names differ per provider (`prompt_tokens` vs `input_tokens`), so
/// the parsers below accept either spelling and the sub-counters simply
/// read 0 on the provider that doesn't report them.
#[derive(Default, Clone, Copy)]
pub struct Usage {
    pub prompt: i64,
    pub completion: i64,
    pub total: i64,
    /// Anthropic: prompt tokens served from the prompt cache (billed cheap).
    pub cache_read: i64,
    /// Anthropic: prompt tokens written into the cache (billed a premium).
    pub cache_creation: i64,
    /// OpenAI: reasoning tokens, a subset of `completion`.
    pub reasoning: i64,
}

impl Usage {
    /// True when nothing billable was reported.
    pub fn is_empty(&self) -> bool {
        self.prompt == 0 && self.completion == 0
    }

    /// Fold a later usage report into this one, field by field.
    ///
    /// Both protocols split the counts across events: Anthropic reports
    /// `input_tokens` in `message_start` and `output_tokens` in a trailing
    /// `message_delta`, and OpenAI reports the breakdown only in the final
    /// chunk. Taking the last report wholesale throws away whatever the
    /// earlier one carried — a streamed request would record zero prompt
    /// tokens on every call, which reads as "free" in the usage report.
    ///
    /// So each field is kept at its largest non-zero value instead. The counts
    /// only ever grow within one response, so max is the right merge for all
    /// of them, cache included.
    pub fn merge(&mut self, other: Usage) {
        self.prompt = self.prompt.max(other.prompt);
        self.completion = self.completion.max(other.completion);
        self.total = self.total.max(other.total);
        self.cache_read = self.cache_read.max(other.cache_read);
        self.cache_creation = self.cache_creation.max(other.cache_creation);
        self.reasoning = self.reasoning.max(other.reasoning);
    }
}

/// Read a token count out of a `usage` object, trying each name in order.
fn usage_field(u: &Value, names: &[&str]) -> i64 {
    names
        .iter()
        .find_map(|n| u.get(*n).and_then(|x| x.as_i64()))
        .unwrap_or(0)
}

/// Build a `Usage` from an upstream `usage` object, accepting both the
/// OpenAI and Anthropic spellings for the shared fields.
pub fn parse_usage_obj(u: &Value) -> Usage {
    let prompt = usage_field(u, &["prompt_tokens", "input_tokens"]);
    let completion = usage_field(u, &["completion_tokens", "output_tokens"]);
    // OpenAI nests it under completion_tokens_details; Anthropic reports no
    // equivalent on the response object.
    let reasoning = u
        .get("completion_tokens_details")
        .map(|d| usage_field(d, &["reasoning_tokens"]))
        .unwrap_or(0);
    // Trust the upstream's total when present, but never let it come out
    // below the parts we summed ourselves.
    let total = usage_field(u, &["total_tokens"]).max(prompt + completion);
    Usage {
        prompt,
        completion,
        total,
        cache_read: usage_field(u, &["cache_read_input_tokens"]),
        cache_creation: usage_field(u, &["cache_creation_input_tokens"]),
        reasoning,
    }
}

/// Pull the token breakdown out of one SSE `data:` payload. Accepts both
/// the OpenAI shape (`{usage:{prompt_tokens}}`) and the Anthropic shape,
/// which spreads it across two events: `message_start` nests it under
/// `message.usage`, `message_delta` puts it at the top level. Returns None
/// for payloads without a usage block — the caller overwrites on each hit,
/// so the last event carrying usage wins (it has the final output count).
pub fn usage_from_sse_payload(payload: &str) -> Option<Usage> {
    let v: Value = serde_json::from_str(payload).ok()?;
    let u = v
        .get("usage")
        .or_else(|| v.get("message").and_then(|m| m.get("usage")))?;
    let parsed = parse_usage_obj(u);
    if parsed.is_empty() {
        return None;
    }
    Some(parsed)
}

/// One streaming converter: feed it `data:` payloads from the upstream SSE
/// (the payload only, without the `data: ` prefix), receive complete
/// client-protocol SSE event blocks (each ending in a blank line).
pub trait SseConverter: Send {
    fn on_data(&mut self, payload: &str) -> Vec<String>;
    /// Emit any remaining events when the upstream stream ends.
    fn finish(&mut self) -> Vec<String>;
    /// Final token breakdown the converter captured from upstream chunks.
    /// None means no usage was reported in the stream — the log row will
    /// record 0 tokens in that case.
    fn usage(&self) -> Option<Usage> {
        None
    }
}

fn sse_event(name: &str, data: &Value) -> String {
    format!("event: {}\ndata: {}\n\n", name, data)
}

fn openai_chunk(id: &str, created: i64, model: &str, delta: Value, finish_reason: Value) -> String {
    let data = json!({
        "id": format!("chatcmpl-{}", id),
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish_reason }]
    });
    format!("data: {}\n\n", data)
}

fn msg_id(id: &str) -> String {
    format!("msg_{}", id.strip_prefix("chatcmpl-").unwrap_or(id))
}

/// One OpenAI tool call, assembled from however many fragments the upstream
/// chose to split it across.
///
/// OpenAI has no hard rule about where `id`, `name` and `arguments` land: the
/// usual shape puts `id` and `name` in the first fragment with an empty
/// `arguments`, but plenty of providers (u2-flash among them) send the whole
/// call — `id`, `name` *and* the complete argument JSON — in one fragment.
/// Anthropic, on the other hand, can only open a `tool_use` block once it
/// knows the name, and `content_block_start` cannot be retracted afterwards.
/// So argument fragments are buffered in `pending` rather than emitted on
/// sight, and flushed as a single `input_json_delta` right after the block
/// opens. Emitting them eagerly would mean dropping the ones that arrive with
/// `id`, and the client then sees a tool call with `input: {}` — an
/// `InputValidationError` on the tool's required parameters.
#[derive(Clone, Default)]
struct ToolSlot {
    /// Set once `content_block_start` has been emitted.
    block_index: Option<usize>,
    id: String,
    name: String,
    /// Argument fragments held back until the block is open.
    pending: String,
}

/// OpenAI chunk SSE -> Anthropic event stream.
pub struct OpenAiToAnthropicStream {
    model: String,
    started: bool,
    next_block: usize,
    text_block: Option<usize>,
    /// Open block index of the `thinking` block, once one has been opened.
    thinking_block: Option<usize>,
    /// per OpenAI tool-call index
    tools: Vec<ToolSlot>,
    finish_reason: Option<String>,
    /// Latest usage block seen upstream. OpenAI streams it once, in the
    /// final chunk when the caller asked for `stream_options.include_usage`.
    usage: Usage,
    id: String,
}

impl OpenAiToAnthropicStream {
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
            started: false,
            next_block: 0,
            text_block: None,
            thinking_block: None,
            tools: Vec::new(),
            finish_reason: None,
            usage: Usage::default(),
            id: format!("stream-{}", db::now()),
        }
    }
}

impl OpenAiToAnthropicStream {
    /// The opening envelope of the translated Anthropic stream. Split out
    /// because `finish()` has to be able to emit it too (see there).
    fn message_start_event(&mut self) -> String {
        self.started = true;
        sse_event(
            "message_start",
            &json!({
                "type": "message_start",
                "message": {
                    "id": msg_id(&self.id), "type": "message", "role": "assistant",
                    "model": self.model, "content": [], "stop_reason": Value::Null,
                    "stop_sequence": Value::Null,
                    "usage": { "input_tokens": 0, "output_tokens": 0 }
                }
            }),
        )
    }

    /// Emit `content_block_start` for tool call `idx` if it isn't open yet.
    /// Returns the block index once the block is open.
    ///
    /// `allow_unnamed` is for the end-of-stream path: a call whose name never
    /// arrived opens anyway (an empty name is malformed, but dropping the call
    /// would discard its arguments in silence), while mid-stream we simply
    /// wait — a later fragment may still carry the name.
    fn ensure_tool_block(
        &mut self,
        idx: usize,
        out: &mut Vec<String>,
        allow_unnamed: bool,
    ) -> Option<usize> {
        if self.tools[idx].block_index.is_none() {
            if self.tools[idx].name.is_empty() && !allow_unnamed {
                return None;
            }
            let block_index = self.next_block;
            self.next_block += 1;
            let id = if self.tools[idx].id.is_empty() {
                format!("toolu_{block_index}")
            } else {
                self.tools[idx].id.clone()
            };
            self.tools[idx].block_index = Some(block_index);
            out.push(sse_event(
                "content_block_start",
                &json!({
                    "type": "content_block_start", "index": block_index,
                    "content_block": {
                        "type": "tool_use",
                        "id": id, "name": self.tools[idx].name, "input": {}
                    }
                }),
            ));
        }
        self.tools[idx].block_index
    }

    /// Forward a `reasoning_content` fragment as an Anthropic `thinking`
    /// block, opening the block on first use.
    ///
    /// The signature is the part worth explaining. Anthropic requires one,
    /// but an OpenAI-format upstream has no such concept — its reasoning is
    /// unsigned by construction, and the relays that expose `reasoning_content`
    /// give nothing to forward. So we mint a stable placeholder. That is only
    /// safe because the block never reaches a real Anthropic service: the
    /// client echoes it back on the next turn and
    /// `anthropic_messages_to_openai` drops it (its `_ => {}` arm), so
    /// nothing downstream ever tries to verify it. If that ever changes —
    /// a client that validates signatures itself, or a passthrough path that
    /// stops dropping the block — this becomes a real forging bug and needs
    /// rethinking rather than a different placeholder.
    ///
    /// Whether the reasoning reaches the client at all is upstream's choice,
    /// not ours: `disable_upstream_thinking` already asks for none, and a
    /// model that honours it never emits a fragment. This is the fallback for
    /// the ones that don't — u2-flash's OpenAI endpoint being the case that
    /// motivated it, where the reasoning still consumes the caller's whole
    /// `max_tokens` and the reply would otherwise arrive as `content: null`.
    fn emit_reasoning(&mut self, reasoning: &str, out: &mut Vec<String>) {
        if reasoning.is_empty() {
            return;
        }
        if self.thinking_block.is_none() {
            let block_index = self.next_block;
            self.next_block += 1;
            out.push(sse_event(
                "content_block_start",
                &json!({
                    "type": "content_block_start", "index": block_index,
                    "content_block": {
                        "type": "thinking", "thinking": "", "signature": ""
                    }
                }),
            ));
            self.thinking_block = Some(block_index);
        }
        out.push(sse_event(
            "content_block_delta",
            &json!({
                "type": "content_block_delta", "index": self.thinking_block,
                "delta": { "type": "thinking_delta", "thinking": reasoning }
            }),
        ));
    }

    /// Forward whatever argument fragments `idx` has buffered, now that the
    /// block they belong to is open.
    fn flush_tool_args(&mut self, idx: usize, out: &mut Vec<String>) {
        if let Some(block_index) = self.tools[idx].block_index {
            let pending = std::mem::take(&mut self.tools[idx].pending);
            if !pending.is_empty() {
                out.push(sse_event(
                    "content_block_delta",
                    &json!({
                        "type": "content_block_delta", "index": block_index,
                        "delta": { "type": "input_json_delta", "partial_json": pending }
                    }),
                ));
            }
        }
    }
}

impl SseConverter for OpenAiToAnthropicStream {
    fn on_data(&mut self, payload: &str) -> Vec<String> {
        let v: Value = match serde_json::from_str(payload) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::new();
        if !self.started {
            if let Some(id) = v.get("id").and_then(|i| i.as_str()) {
                self.id = id.to_string();
            }
            out.push(self.message_start_event());
        }
        // capture usage if the upstream includes it (final chunks often do)
        if let Some(u) = v.get("usage") {
            let parsed = parse_usage_obj(u);
            if !parsed.is_empty() {
                self.usage = parsed;
            }
        }
        let choice = match v.get("choices").and_then(|c| c.get(0)) {
            Some(c) => c,
            None => return out,
        };
        let delta = choice.get("delta");
        // tool call fragments
        if let Some(calls) = delta
            .and_then(|d| d.get("tool_calls"))
            .and_then(|c| c.as_array())
        {
            for call in calls {
                let idx = call.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                if self.tools.len() <= idx {
                    self.tools.resize(idx + 1, ToolSlot::default());
                }
                let function = call.get("function");
                let name = function
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                let args = function
                    .and_then(|f| f.get("arguments"))
                    .and_then(|a| a.as_str())
                    .unwrap_or("");
                {
                    let slot = &mut self.tools[idx];
                    if let Some(id) = call.get("id").and_then(|i| i.as_str()) {
                        if !id.is_empty() {
                            slot.id = id.to_string();
                        }
                    }
                    if !name.is_empty() {
                        slot.name = name.to_string();
                    }
                    // Buffered, not emitted on sight — see `ToolSlot`.
                    if !args.is_empty() {
                        slot.pending.push_str(args);
                    }
                }
                self.ensure_tool_block(idx, &mut out, false);
                self.flush_tool_args(idx, &mut out);
            }
        }
        // reasoning fragments — an OpenAI upstream's thinking, translated
        if let Some(reasoning) = delta
            .and_then(|d| d.get("reasoning_content"))
            .and_then(|r| r.as_str())
        {
            self.emit_reasoning(reasoning, &mut out);
        }
        // text fragments
        if let Some(text) = delta
            .and_then(|d| d.get("content"))
            .and_then(|c| c.as_str())
        {
            if !text.is_empty() {
                if self.text_block.is_none() {
                    let block_index = self.next_block;
                    self.next_block += 1;
                    out.push(sse_event(
                        "content_block_start",
                        &json!({
                            "type": "content_block_start", "index": block_index,
                            "content_block": { "type": "text", "text": "" }
                        }),
                    ));
                    self.text_block = Some(block_index);
                }
                out.push(sse_event(
                    "content_block_delta",
                    &json!({
                        "type": "content_block_delta", "index": self.text_block,
                        "delta": { "type": "text_delta", "text": text }
                    }),
                ));
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
            self.finish_reason = Some(reason.to_string());
        }
        out
    }

    fn finish(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        // An upstream that opened a stream and then sent nothing we could
        // translate leaves `started` false. Emitting `message_delta` /
        // `message_stop` without a preceding `message_start` is a malformed
        // Anthropic sequence that strict clients reject, so open the message
        // first — the empty one is still the honest representation.
        if !self.started {
            out.push(self.message_start_event());
        }
        // A tool call whose name never arrived opens now, so the arguments
        // already buffered for it reach the client instead of disappearing.
        for idx in 0..self.tools.len() {
            self.ensure_tool_block(idx, &mut out, true);
            self.flush_tool_args(idx, &mut out);
        }
        // close open blocks in ascending block order
        let mut open: Vec<usize> = self
            .tools
            .iter()
            .filter_map(|slot| slot.block_index)
            .collect();
        if let Some(t) = self.text_block {
            open.push(t);
        }
        if let Some(t) = self.thinking_block {
            open.push(t);
        }
        open.sort_unstable();
        for idx in open {
            out.push(sse_event(
                "content_block_stop",
                &json!({ "type": "content_block_stop", "index": idx }),
            ));
        }
        let reason = self
            .finish_reason
            .as_deref()
            .map(finish_openai_to_anthropic)
            .unwrap_or("end_turn");
        out.push(sse_event(
            "message_delta",
            &json!({
                "type": "message_delta",
                "delta": { "stop_reason": reason, "stop_sequence": Value::Null },
                "usage": { "input_tokens": self.usage.prompt, "output_tokens": self.usage.completion }
            }),
        ));
        out.push(sse_event(
            "message_stop",
            &json!({ "type": "message_stop" }),
        ));
        out
    }

    fn usage(&self) -> Option<Usage> {
        if self.usage.is_empty() {
            None
        } else {
            Some(self.usage)
        }
    }
}

/// Anthropic event SSE -> OpenAI chunk stream.
pub struct AnthropicToOpenAiStream {
    model: String,
    created: i64,
    id: String,
    /// Index of the current tool block whose delta events are arriving.
    /// Set by `content_block_start`, reset on the next `content_block_start`.
    tool_index: usize,
    /// Total number of tool_use blocks seen so far — also the index of the
    /// next block to open.
    tool_count: usize,
    finish_reason: Option<String>,
    /// Accumulated usage. Anthropic streams it in two places: `message_start`
    /// carries the input counts, `message_delta` the final output count, so
    /// each update only overwrites the fields it actually reports.
    usage: Usage,
}

impl AnthropicToOpenAiStream {
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
            created: db::now(),
            id: format!("stream-{}", db::now()),
            tool_index: 0,
            tool_count: 0,
            finish_reason: None,
            usage: Usage::default(),
        }
    }
}

impl SseConverter for AnthropicToOpenAiStream {
    fn on_data(&mut self, payload: &str) -> Vec<String> {
        let v: Value = match serde_json::from_str(payload) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::new();
        match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "message_start" => {
                if let Some(id) = v
                    .get("message")
                    .and_then(|m| m.get("id"))
                    .and_then(|i| i.as_str())
                {
                    self.id = id.strip_prefix("msg_").unwrap_or(id).to_string();
                }
                // message_start is the only place the input counts appear;
                // message_delta only carries the final output count.
                if let Some(u) = v.get("message").and_then(|m| m.get("usage")) {
                    let parsed = parse_usage_obj(u);
                    if parsed.prompt > 0 {
                        self.usage.prompt = parsed.prompt;
                    }
                    if parsed.cache_read > 0 {
                        self.usage.cache_read = parsed.cache_read;
                    }
                    if parsed.cache_creation > 0 {
                        self.usage.cache_creation = parsed.cache_creation;
                    }
                }
                out.push(openai_chunk(
                    &self.id,
                    self.created,
                    &self.model,
                    json!({ "role": "assistant" }),
                    Value::Null,
                ));
            }
            "content_block_start" => {
                let block = v.get("content_block").cloned().unwrap_or_else(|| json!({}));
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    self.tool_index = self.tool_count;
                    self.tool_count += 1;
                    out.push(openai_chunk(
                        &self.id,
                        self.created,
                        &self.model,
                        json!({
                            "tool_calls": [{
                                "index": self.tool_index,
                                "id": block.get("id").cloned().unwrap_or(Value::Null),
                                "type": "function",
                                "function": {
                                    "name": block.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                                    "arguments": ""
                                }
                            }]
                        }),
                        Value::Null,
                    ));
                }
            }
            "content_block_delta" => {
                let delta = v.get("delta").cloned().unwrap_or_else(|| json!({}));
                match delta.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "text_delta" => {
                        out.push(openai_chunk(
                            &self.id,
                            self.created,
                            &self.model,
                            json!({ "content": delta.get("text").and_then(|t| t.as_str()).unwrap_or("") }),
                            Value::Null,
                        ));
                    }
                    "input_json_delta" => {
                        out.push(openai_chunk(
                            &self.id,
                            self.created,
                            &self.model,
                            json!({
                                "tool_calls": [{
                                    "index": self.tool_index,
                                    "function": {
                                        "arguments": delta.get("partial_json").and_then(|p| p.as_str()).unwrap_or("")
                                    }
                                }]
                            }),
                            Value::Null,
                        ));
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(r) = v
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(|r| r.as_str())
                {
                    self.finish_reason = Some(r.to_string());
                }
                if let Some(u) = v.get("usage") {
                    let parsed = parse_usage_obj(u);
                    if parsed.completion > 0 {
                        self.usage.completion = parsed.completion;
                    }
                    if parsed.prompt > 0 {
                        self.usage.prompt = parsed.prompt;
                    }
                }
            }
            _ => {}
        }
        out
    }

    fn finish(&mut self) -> Vec<String> {
        let reason = self
            .finish_reason
            .as_deref()
            .map(finish_anthropic_to_openai)
            .unwrap_or("stop");
        let mut out = vec![openai_chunk(
            &self.id,
            self.created,
            &self.model,
            json!({}),
            json!(reason),
        )];
        // final usage chunk (mirrors openai's include_usage stream tail)
        out.push(format!(
            "data: {}\n\n",
            json!({
                "id": format!("chatcmpl-{}", self.id),
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.model,
                "choices": [],
                "usage": {
                    "prompt_tokens": self.usage.prompt,
                    "completion_tokens": self.usage.completion,
                    "total_tokens": self.usage.total
                }
            })
        ));
        out.push("data: [DONE]\n\n".to_string());
        out
    }

    fn usage(&self) -> Option<Usage> {
        if self.usage.is_empty() {
            None
        } else {
            Some(self.usage)
        }
    }
}
