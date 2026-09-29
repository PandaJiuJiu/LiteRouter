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
                            let url = match source.and_then(|s| s.get("type")).and_then(|t| t.as_str())
                            {
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
                                parts.push(json!({ "type": "image_url", "image_url": { "url": url } }));
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
                            c.as_str().map(|s| s.to_string()).unwrap_or_else(|| c.to_string())
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
    openai_messages_to_anthropic(req.get("messages").unwrap_or(&Value::Null), &mut messages, &mut system);
    let mut out = json!({ "model": model, "messages": messages });
    if !system.is_empty() {
        out["system"] = Value::String(system);
    }
    out["max_tokens"] = req.get("max_tokens").cloned().unwrap_or_else(|| json!(4096));
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

/// OpenAI non-streaming response -> Anthropic `message` shape.
pub fn openai_resp_to_anthropic(body: &Value, model: &str) -> Value {
    let choice = body.get("choices").and_then(|c| c.get(0));
    let message = choice.and_then(|c| c.get("message"));
    let mut content: Vec<Value> = Vec::new();
    if let Some(text) = message
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
    {
        if !text.is_empty() {
            content.push(json!({ "type": "text", "text": text }));
        }
    }
    if let Some(calls) = message.and_then(|m| m.get("tool_calls")).and_then(|c| c.as_array()) {
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
    for b in body.get("content").and_then(|c| c.as_array()).into_iter().flatten() {
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
        json!({ "role": "assistant", "content": text.is_empty().then_some(Value::Null).unwrap_or(Value::String(text)), "tool_calls": tool_calls })
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
        "id": format!("chatcmpl-{}", body.get("id").and_then(|i| i.as_str()).unwrap_or("unknown")),
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

/// One streaming converter: feed it `data:` payloads from the upstream SSE
/// (the payload only, without the `data: ` prefix), receive complete
/// client-protocol SSE event blocks (each ending in a blank line).
pub trait SseConverter: Send {
    fn on_data(&mut self, payload: &str) -> Vec<String>;
    /// Emit any remaining events when the upstream stream ends.
    fn finish(&mut self) -> Vec<String>;
    /// Final (prompt, completion) token counts the converter captured from
    /// upstream chunks. None means no usage was reported in the stream — the
    /// log row will record 0 tokens in that case.
    fn usage(&self) -> Option<(i64, i64)> { None }
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

/// OpenAI chunk SSE -> Anthropic event stream.
pub struct OpenAiToAnthropicStream {
    model: String,
    started: bool,
    next_block: usize,
    text_block: Option<usize>,
    /// per OpenAI tool-call index: (block index, started, name)
    tools: Vec<Option<(usize, bool, String)>>,
    finish_reason: Option<String>,
    usage: (i64, i64), // (input, output) seen so far
    id: String,
}

impl OpenAiToAnthropicStream {
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
            started: false,
            next_block: 0,
            text_block: None,
            tools: Vec::new(),
            finish_reason: None,
            usage: (0, 0),
            id: format!("stream-{}", crate::db::now()),
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
            self.started = true;
            if let Some(id) = v.get("id").and_then(|i| i.as_str()) {
                self.id = id.to_string();
            }
            out.push(sse_event(
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
            ));
        }
        // capture usage if the upstream includes it (final chunks often do)
        if let Some(u) = v.get("usage").and_then(|u| u.as_object()) {
            if let Some(p) = u.get("prompt_tokens").and_then(|x| x.as_i64()) {
                self.usage.0 = p;
            }
            if let Some(c) = u.get("completion_tokens").and_then(|x| x.as_i64()) {
                self.usage.1 = c;
            }
        }
        let choice = match v.get("choices").and_then(|c| c.get(0)) {
            Some(c) => c,
            None => return out,
        };
        let delta = choice.get("delta");
        // tool call fragments
        if let Some(calls) = delta.and_then(|d| d.get("tool_calls")).and_then(|c| c.as_array()) {
            for call in calls {
                let idx = call
                    .get("index")
                    .and_then(|i| i.as_u64())
                    .unwrap_or(0) as usize;
                if self.tools.len() <= idx {
                    self.tools.resize(idx + 1, None);
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
                let is_new = call.get("id").is_some();
                if is_new && self.tools[idx].is_none() {
                    let block_index = self.next_block;
                    self.next_block += 1;
                    out.push(sse_event(
                        "content_block_start",
                        &json!({
                            "type": "content_block_start", "index": block_index,
                            "content_block": {
                                "type": "tool_use",
                                "id": call.get("id").cloned().unwrap_or(Value::Null),
                                "name": name, "input": {}
                            }
                        }),
                    ));
                    self.tools[idx] = Some((block_index, true, name.to_string()));
                } else if !args.is_empty() {
                    match self.tools[idx] {
                        // mid-stream fragments of an already-started block
                        Some((block_index, true, _)) => out.push(sse_event(
                            "content_block_delta",
                            &json!({
                                "type": "content_block_delta", "index": block_index,
                                "delta": { "type": "input_json_delta", "partial_json": args }
                            }),
                        )),
                        // fragments before we saw an id: start the block now
                        None => {
                            let block_index = self.next_block;
                            self.next_block += 1;
                            out.push(sse_event(
                                "content_block_start",
                                &json!({
                                    "type": "content_block_start", "index": block_index,
                                    "content_block": {
                                        "type": "tool_use",
                                        "id": call.get("id").cloned().unwrap_or_else(|| json!(format!("toolu_{}", block_index))),
                                        "name": name, "input": {}
                                    }
                                }),
                            ));
                            out.push(sse_event(
                                "content_block_delta",
                                &json!({
                                    "type": "content_block_delta", "index": block_index,
                                    "delta": { "type": "input_json_delta", "partial_json": args }
                                }),
                            ));
                            self.tools[idx] = Some((block_index, true, name.to_string()));
                        }
                        _ => {}
                    }
                }
            }
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
        // close open blocks in ascending block order
        let mut open: Vec<usize> = self
            .tools
            .iter()
            .flatten()
            .map(|(idx, _, _)| *idx)
            .collect();
        if let Some(t) = self.text_block {
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
                "usage": { "input_tokens": self.usage.0, "output_tokens": self.usage.1 }
            }),
        ));
        out.push(sse_event("message_stop", &json!({ "type": "message_stop" })));
        out
    }

    fn usage(&self) -> Option<(i64, i64)> {
        // OpenAI's chunk.usage uses prompt_tokens / completion_tokens. We've
        // been storing them as-is into self.usage, so we can just return it
        // whenever any non-zero count was observed.
        if self.usage.0 > 0 || self.usage.1 > 0 {
            Some(self.usage)
        } else {
            None
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
    usage: (i64, i64), // (input, output)
}

impl AnthropicToOpenAiStream {
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
            created: crate::db::now(),
            id: format!("stream-{}", crate::db::now()),
            tool_count: 0,
            finish_reason: None,
            usage: (0, 0),
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
                if let Some(r) = v.get("delta").and_then(|d| d.get("stop_reason")).and_then(|r| r.as_str()) {
                    self.finish_reason = Some(r.to_string());
                }
                if let Some(u) = v.get("usage") {
                    if let Some(i) = u.get("input_tokens").and_then(|x| x.as_i64()) {
                        self.usage.0 = i;
                    }
                    if let Some(o) = u.get("output_tokens").and_then(|x| x.as_i64()) {
                        self.usage.1 = o;
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
                    "prompt_tokens": self.usage.0,
                    "completion_tokens": self.usage.1,
                    "total_tokens": self.usage.0 + self.usage.1
                }
            })
        ));
        out.push("data: [DONE]\n\n".to_string());
        out
    }

    fn usage(&self) -> Option<(i64, i64)> {
        // Anthropic SSE reports input_tokens / output_tokens. We copy them
        // straight into self.usage, so return as-is when any non-zero.
        if self.usage.0 > 0 || self.usage.1 > 0 {
            Some(self.usage)
        } else {
            None
        }
    }
}
