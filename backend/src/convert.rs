//! Bidirectional protocol conversion: Anthropic Messages ⇄ OpenAI
//! ChatCompletions. When a client speaks one protocol but the channel only
//! serves the other, the relay converts requests before forwarding and
//! converts responses (buffered and streaming) back to the client's shape.
//! Mapping tables follow one-api's anthropic adaptor
//! (relay/adaptor/anthropic/main.go), inverted where needed.

use serde_json::{json, Value};

/// Which protocol translation a given channel hop needs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConvertMode {
    /// channel natively speaks the client's protocol
    None,
    /// anthropic client -> openai upstream
    ToOpenAI,
    /// openai client -> anthropic upstream
    ToAnthropic,
}

// =========================== requests ===========================

/// Concatenate an anthropic `system` value (string or block array) to text.
fn system_to_text(sys: &Value) -> String {
    match sys {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Fold openai content parts into `content`: single text -> plain string,
/// empty -> null, otherwise the parts array.
fn parts_to_content(parts: Vec<Value>) -> Value {
    match parts.len() {
        0 => Value::Null,
        1 if parts[0].get("type").and_then(|t| t.as_str()) == Some("text") => {
            parts[0].get("text").cloned().unwrap_or(Value::Null)
        }
        _ => Value::Array(parts),
    }
}

/// Anthropic messages -> OpenAI messages. Tool results become separate
/// `role:"tool"` messages; tool_use becomes `tool_calls` on the assistant
/// message; images become data-URI (or remote) `image_url` parts.
fn anthropic_messages_to_openai(messages: &Value, out: &mut Vec<Value>) {
    for m in messages.as_array().into_iter().flatten() {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let mut parts: Vec<Value> = Vec::new();
        let mut tool_calls: Vec<Value> = Vec::new();
        let mut tool_results: Vec<Value> = Vec::new();
        match m.get("content") {
            Some(Value::String(s)) => parts.push(json!({ "type": "text", "text": s })),
            Some(Value::Array(blocks)) => {
                for b in blocks {
                    match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                        "text" => {
                            if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                                parts.push(json!({ "type": "text", "text": t }));
                            }
                        }
                        "image" => {
                            let source = b.get("source");
                            let url =
                                match source.and_then(|s| s.get("type")).and_then(|t| t.as_str()) {
                                    Some("base64") => {
                                        let media = source
                                            .and_then(|s| s.get("media_type"))
                                            .and_then(|m| m.as_str())
                                            .unwrap_or("image/png");
                                        let data = source
                                            .and_then(|s| s.get("data"))
                                            .and_then(|d| d.as_str())
                                            .unwrap_or("");
                                        format!("data:{};base64,{}", media, data)
                                    }
                                    Some("url") => source
                                        .and_then(|s| s.get("url"))
                                        .and_then(|u| u.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    _ => continue,
                                };
                            if !url.is_empty() {
                                parts.push(
                                    json!({ "type": "image_url", "image_url": { "url": url } }),
                                );
                            }
                        }
                        "tool_use" => {
                            tool_calls.push(json!({
                                "id": b.get("id").cloned().unwrap_or(Value::Null),
                                "type": "function",
                                "function": {
                                    "name": b.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                                    "arguments": b.get("input").map(|i| i.to_string()).unwrap_or_else(|| "{}".into()),
                                }
                            }));
                        }
                        "tool_result" => {
                            // content may be a string or a block array; keep text
                            let content = match b.get("content") {
                                Some(Value::String(s)) => Value::String(s.clone()),
                                Some(Value::Array(cblocks)) => Value::String(
                                    cblocks
                                        .iter()
                                        .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                ),
                                _ => Value::String(String::new()),
                            };
                            tool_results.push(json!({
                                "role": "tool",
                                "tool_call_id": b.get("tool_use_id").cloned().unwrap_or(Value::Null),
                                "content": content,
                            }));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        // OpenAI expects tool result messages directly after the assistant
        // message that made the calls.
        out.extend(tool_results);
        if role == "assistant" {
            let mut msg = json!({ "role": "assistant" });
            let content = parts_to_content(parts);
            if !content.is_null() {
                msg["content"] = content;
            }
            if !tool_calls.is_empty() {
                msg["tool_calls"] = Value::Array(tool_calls);
            }
            out.push(msg);
        } else {
            if parts.is_empty() {
                continue; // pure tool_result message, already emitted above
            }
            out.push(json!({ "role": "user", "content": parts_to_content(parts) }));
        }
    }
}

/// Anthropic Messages request -> OpenAI ChatCompletions request.
pub fn anthropic_req_to_openai(req: &Value, model: &str) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(sys) = req.get("system") {
        let text = system_to_text(sys);
        if !text.is_empty() {
            messages.push(json!({ "role": "system", "content": text }));
        }
    }
    anthropic_messages_to_openai(req.get("messages").unwrap_or(&Value::Null), &mut messages);
    let mut out = json!({ "model": model, "messages": messages });
    // anthropic requires max_tokens; openai defaults it if absent
    out["max_tokens"] = req
        .get("max_tokens")
        .cloned()
        .unwrap_or_else(|| json!(4096));
    if let Some(v) = req.get("temperature") {
        out["temperature"] = v.clone();
    }
    if let Some(v) = req.get("top_p") {
        out["top_p"] = v.clone();
    }
    if let Some(stops) = req.get("stop_sequences").and_then(|s| s.as_array()) {
        out["stop"] = Value::Array(stops.clone());
    }
    if let Some(stream) = req.get("stream") {
        out["stream"] = stream.clone();
        // OpenAI omits usage from a streamed response unless asked for it, and
        // `stream_options` is the only way to ask. Without this the upstream
        // stream carries no token counts at all and every streamed log row
        // lands on zero — the Anthropic side reports usage natively, which is
        // what makes this asymmetry easy to miss. Anthropic has no equivalent
        // switch, so `openai_req_to_anthropic` deliberately has no counterpart.
        if stream == &json!(true) {
            out["stream_options"] = json!({ "include_usage": true });
        }
    }
    if let Some(tools) = req.get("tools").and_then(|t| t.as_array()) {
        let mapped: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                        "description": t.get("description").and_then(|d| d.as_str()).unwrap_or(""),
                        "parameters": t.get("input_schema").cloned().unwrap_or_else(|| json!({"type":"object"})),
                    }
                })
            })
            .collect();
        if !mapped.is_empty() {
            out["tools"] = Value::Array(mapped);
            // anthropic tool_choice: "auto" | "any" | {"type":"tool","name"}
            match req.get("tool_choice") {
                Some(Value::String(s)) => {
                    if s == "any" {
                        out["tool_choice"] = json!("required");
                    } else {
                        out["tool_choice"] = json!("auto");
                    }
                }
                Some(c) if c.get("type").and_then(|t| t.as_str()) == Some("tool") => {
                    out["tool_choice"] = json!({
                        "type": "function",
                        "function": { "name": c.get("name").and_then(|n| n.as_str()).unwrap_or("") }
                    });
                }
                _ => out["tool_choice"] = json!("auto"),
            }
        }
    }
    out
}

/// OpenAI messages -> Anthropic messages (reverse of the above).
fn openai_messages_to_anthropic(messages: &Value, out: &mut Vec<Value>, system: &mut String) {
    for m in messages.as_array().into_iter().flatten() {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let mut blocks: Vec<Value> = Vec::new();
        match m.get("content") {
            Some(Value::String(s)) if !s.is_empty() => {
                blocks.push(json!({ "type": "text", "text": s }));
            }
            Some(Value::Array(parts)) => {
                for p in parts {
                    match p.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                        "text" => blocks.push(json!({
                            "type": "text",
                            "text": p.get("text").and_then(|t| t.as_str()).unwrap_or("")
                        })),
                        "image_url" => {
                            let url = p
                                .get("image_url")
                                .and_then(|i| i.get("url"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("");
                            if let Some(data) = url.strip_prefix("data:") {
                                // data:<media>;base64,<data>
                                let (media, data) = match data.split_once(";base64,") {
                                    Some((m, d)) => (m, d),
                                    None => ("image/png", data),
                                };
                                blocks.push(json!({
                                    "type": "image",
                                    "source": { "type": "base64", "media_type": media, "data": data }
                                }));
                            } else if !url.is_empty() {
                                blocks.push(json!({
                                    "type": "image",
                                    "source": { "type": "url", "url": url }
                                }));
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        match role {
            "system" => {
                if system.is_empty() {
                    *system = m
                        .get("content")
                        .map(|c| {
                            c.as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| c.to_string())
                        })
                        .unwrap_or_default();
                }
                continue;
            }
            "tool" => {
                // becomes a user turn carrying a tool_result block
                let content = m.get("content").cloned().unwrap_or(Value::Null);
                out.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": m.get("tool_call_id").cloned().unwrap_or(Value::Null),
                        "content": content,
                    }]
                }));
                continue;
            }
            _ => {}
        }
        if role == "assistant" {
            if let Some(calls) = m.get("tool_calls").and_then(|c| c.as_array()) {
                for c in calls {
                    let input: Value = c
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .and_then(|a| a.as_str())
                        .and_then(|a| serde_json::from_str(a).ok())
                        .unwrap_or_else(|| json!({}));
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": c.get("id").cloned().unwrap_or(Value::Null),
                        "name": c.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()).unwrap_or(""),
                        "input": input,
                    }));
                }
            }
            if !blocks.is_empty() {
                out.push(json!({ "role": "assistant", "content": blocks }));
            }
        } else if !blocks.is_empty() {
            out.push(json!({ "role": "user", "content": blocks }));
        }
    }
}

/// OpenAI ChatCompletions request -> Anthropic Messages request.
pub fn openai_req_to_anthropic(req: &Value, model: &str) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    let mut system = String::new();
    openai_messages_to_anthropic(
        req.get("messages").unwrap_or(&Value::Null),
        &mut messages,
        &mut system,
    );
    let mut out = json!({ "model": model, "messages": messages });
    if !system.is_empty() {
        out["system"] = Value::String(system);
    }
    out["max_tokens"] = req
        .get("max_tokens")
        .cloned()
        .unwrap_or_else(|| json!(4096));
    if let Some(v) = req.get("temperature") {
        out["temperature"] = v.clone();
    }
    if let Some(v) = req.get("top_p") {
        out["top_p"] = v.clone();
    }
    if let Some(stops) = req.get("stop").and_then(|s| s.as_array()) {
        out["stop_sequences"] = Value::Array(stops.clone());
    }
    if let Some(stream) = req.get("stream") {
        out["stream"] = stream.clone();
    }
    if let Some(tools) = req.get("tools").and_then(|t| t.as_array()) {
        let mapped: Vec<Value> = tools
            .iter()
            .filter_map(|t| {
                let f = t.get("function")?;
                Some(json!({
                    "name": f.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                    "description": f.get("description").and_then(|d| d.as_str()).unwrap_or(""),
                    "input_schema": f.get("parameters").cloned().unwrap_or_else(|| json!({"type":"object"})),
                }))
            })
            .collect();
        if !mapped.is_empty() {
            out["tools"] = Value::Array(mapped);
            match req.get("tool_choice") {
                Some(Value::String(s)) if s == "required" => {
                    out["tool_choice"] = json!({ "type": "any" });
                }
                Some(c) if c.get("type").and_then(|t| t.as_str()) == Some("function") => {
                    out["tool_choice"] = json!({
                        "type": "tool",
                        "name": c.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()).unwrap_or("")
                    });
                }
                _ => out["tool_choice"] = json!({ "type": "auto" }),
            }
        }
    }
    out
}

// =========================== responses (non-streaming) ===========================

fn finish_openai_to_anthropic(reason: &str) -> &'static str {
    match reason {
        "stop" => "end_turn",
        "length" => "max_tokens",
        "tool_calls" | "function_call" => "tool_use",
        "stop_sequence" => "stop_sequence",
        _ => "end_turn",
    }
}

fn finish_anthropic_to_openai(reason: &str) -> &'static str {
    match reason {
        "end_turn" | "stop_sequence" => "stop",
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        _ => "stop",
    }
}

fn msg_id(id: &str) -> String {
    format!("msg_{}", id.strip_prefix("chatcmpl-").unwrap_or(id))
}

/// Inverse of [`msg_id`]. Also strips `msg_` so the buffered and streaming
/// paths produce the same id for the same upstream response — the streaming
/// converter does this too, and a client that saw `chatcmpl-abc` mid-stream
/// should not see `chatcmpl-msg_abc` in the final buffered shape.
fn openai_id(id: &str) -> String {
    format!("chatcmpl-{}", id.strip_prefix("msg_").unwrap_or(id))
}

/// OpenAI non-streaming response -> Anthropic `message` shape.
pub fn openai_resp_to_anthropic(body: &Value, model: &str) -> Value {
    let choice = body.get("choices").and_then(|c| c.get(0));
    let message = choice.and_then(|c| c.get("message"));
    let mut content: Vec<Value> = Vec::new();
    // Same reasoning-to-thinking mapping as the streaming converter; see
    // `OpenAiToAnthropicStream::emit_reasoning` for why the empty signature
    // is safe here.
    if let Some(reasoning) = message
        .and_then(|m| m.get("reasoning_content"))
        .and_then(|r| r.as_str())
    {
        if !reasoning.is_empty() {
            content.push(json!({
                "type": "thinking", "thinking": reasoning, "signature": ""
            }));
        }
    }
    if let Some(text) = message
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
    {
        if !text.is_empty() {
            content.push(json!({ "type": "text", "text": text }));
        }
    }
    if let Some(calls) = message
        .and_then(|m| m.get("tool_calls"))
        .and_then(|c| c.as_array())
    {
        for c in calls {
            let input: Value = c
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
                .and_then(|a| serde_json::from_str(a).ok())
                .unwrap_or_else(|| json!({}));
            content.push(json!({
                "type": "tool_use",
                "id": c.get("id").cloned().unwrap_or(Value::Null),
                "name": c.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()).unwrap_or(""),
                "input": input,
            }));
        }
    }
    let stop_reason = choice
        .and_then(|c| c.get("finish_reason"))
        .and_then(|r| r.as_str())
        .map(finish_openai_to_anthropic)
        .unwrap_or("end_turn");
    json!({
        "id": msg_id(body.get("id").and_then(|i| i.as_str()).unwrap_or("unknown")),
        "type": "message",
        "role": "assistant",
        "model": body.get("model").and_then(|m| m.as_str()).unwrap_or(model),
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": Value::Null,
        "usage": {
            "input_tokens": body.get("usage").and_then(|u| u.get("prompt_tokens")).and_then(|v| v.as_i64()).unwrap_or(0),
            "output_tokens": body.get("usage").and_then(|u| u.get("completion_tokens")).and_then(|v| v.as_i64()).unwrap_or(0),
        }
    })
}

/// Anthropic non-streaming `message` -> OpenAI TextResponse shape.
pub fn anthropic_resp_to_openai(body: &Value, model: &str) -> Value {
    let mut text = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    for b in body
        .get("content")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
    {
        match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "text" => text.push_str(b.get("text").and_then(|t| t.as_str()).unwrap_or("")),
            "tool_use" => {
                tool_calls.push(json!({
                    "id": b.get("id").cloned().unwrap_or(Value::Null),
                    "type": "function",
                    "function": {
                        "name": b.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                        "arguments": b.get("input").map(|i| i.to_string()).unwrap_or_else(|| "{}".into()),
                    }
                }));
            }
            _ => {}
        }
    }
    let stop_reason = body
        .get("stop_reason")
        .and_then(|r| r.as_str())
        .map(finish_anthropic_to_openai)
        .unwrap_or("stop");
    let message = if tool_calls.is_empty() {
        json!({ "role": "assistant", "content": text })
    } else {
        json!({ "role": "assistant", "content": if text.is_empty() { Value::Null } else { Value::String(text) }, "tool_calls": tool_calls })
    };
    let input = body
        .get("usage")
        .and_then(|u| u.get("input_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let output = body
        .get("usage")
        .and_then(|u| u.get("output_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    json!({
        "id": openai_id(body.get("id").and_then(|i| i.as_str()).unwrap_or("unknown")),
        "object": "chat.completion",
        "created": crate::db::now(),
        "model": body.get("model").and_then(|m| m.as_str()).unwrap_or(model),
        "choices": [{ "index": 0, "message": message, "finish_reason": stop_reason }],
        "usage": { "prompt_tokens": input, "completion_tokens": output, "total_tokens": input + output }
    })
}

// =========================== errors ===========================

/// OpenAI error body -> Anthropic error shape.
pub fn openai_err_to_anthropic(body: &Value) -> Value {
    let err = body.get("error").cloned().unwrap_or_else(|| json!({}));
    json!({
        "type": "error",
        "error": {
            "type": err.get("type").and_then(|t| t.as_str()).unwrap_or("api_error"),
            "message": err.get("message").and_then(|m| m.as_str()).unwrap_or("upstream error"),
        }
    })
}

/// Anthropic error body -> OpenAI error shape.
pub fn anthropic_err_to_openai(body: &Value) -> Value {
    let err = body.get("error").cloned().unwrap_or_else(|| json!({}));
    json!({
        "error": {
            "message": err.get("message").and_then(|m| m.as_str()).unwrap_or("upstream error"),
            "type": err.get("type").and_then(|t| t.as_str()).unwrap_or("api_error"),
            "code": err.get("type").and_then(|t| t.as_str()).unwrap_or("api_error"),
        }
    })
}

// =========================== streaming ===========================

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
            id: format!("stream-{}", crate::db::now()),
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
    tool_count: usize, // number of tool_use blocks seen -> openai tool index
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
            created: crate::db::now(),
            id: format!("stream-{}", crate::db::now()),
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
                    out.push(openai_chunk(
                        &self.id,
                        self.created,
                        &self.model,
                        json!({
                            "tool_calls": [{
                                "index": self.tool_count,
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
                    self.tool_count += 1;
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
                                    "index": self.tool_count.saturating_sub(1),
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
