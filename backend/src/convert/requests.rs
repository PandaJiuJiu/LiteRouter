// =========================== requests ===========================

use super::{json, Value};

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

/// The effort levels both protocols agree on.
///
/// Anthropic's `output_config.effort` and OpenAI's `reasoning_effort` are
/// the same scale with the same names, so a value from one is meaningful to
/// the other. The set is closed on purpose: an unrecognised value is
/// dropped rather than forwarded, because an upstream that doesn't know
/// the level answers 400 ("Invalid option: expected one of ...") and the
/// request dies on a field the gateway introduced.
const EFFORT_LEVELS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

/// Anthropic's thinking configuration -> the OpenAI `reasoning_effort` level.
///
/// None means the client didn't say anything about thinking, and the
/// gateway must not say something on its behalf: the upstream's own
/// default is the right answer for "unspecified".
///
/// `output_config.effort` wins over `thinking` when both are present. The
/// effort level is the precise control — Anthropic's own docs make it the
/// recommended way to tune thinking depth — while `thinking` is only an
/// on/off switch, and one that no longer carries a size on the models that
/// matter: `budget_tokens` is deprecated on the 4.6+ generation, and
/// `{type: "adaptive"}` means "the model decides", which no OpenAI level
/// names exactly.
///
/// The switch maps onto the levels as:
///
/// | `thinking.type` | level | why not something closer |
/// |---|---|---|
/// | `adaptive` | `medium` | "the model decides" has no exact level |
/// | `enabled` | `medium` | `budget_tokens` is a token count; a level is not |
/// | `disabled` | `minimal` | `none` is the exact spelling, but upstreams that require reasoning reject it with a 400 |
///
/// That last row is the important one. `reasoning_effort: "none"` is what
/// "disabled" really means, and MiniMax's endpoint answers it with
/// "Reasoning is mandatory for this endpoint and cannot be disabled" —
/// so the exact translation is the one that breaks the request, and the
/// lowest level that still works is the honest approximation.
fn thinking_to_reasoning_effort(req: &Value) -> Option<&'static str> {
    if let Some(effort) = req
        .get("output_config")
        .and_then(|c| c.get("effort"))
        .and_then(|e| e.as_str())
    {
        if let Some(level) = EFFORT_LEVELS.iter().find(|l| **l == effort) {
            return Some(level);
        }
        // An unknown level is dropped, but the thinking switch below may
        // still say something useful.
    }
    match req
        .get("thinking")
        .and_then(|t| t.get("type"))
        .and_then(|t| t.as_str())
    {
        Some("adaptive") | Some("enabled") => Some("medium"),
        Some("disabled") => Some("minimal"),
        _ => None,
    }
}

/// OpenAI's reasoning controls -> Anthropic's `output_config.effort`.
///
/// Only the effort level crosses. `enable_thinking` deliberately does not:
/// its "off" spelling is `thinking: {type: "disabled"}`, and Anthropic
/// upstreams reject that outright — "thinking.type `disabled` is not
/// supported by this model" — so translating it would turn a request that
/// works into one that fails.
fn reasoning_effort_to_output_config(req: &Value) -> Option<&'static str> {
    let effort = req.get("reasoning_effort").and_then(|e| e.as_str())?;
    EFFORT_LEVELS.iter().find(|l| **l == effort).copied()
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
    // The client's thinking intent, translated. A client that asked for a
    // particular depth of thinking gets that depth upstream instead of
    // the upstream's own default, and a client that said nothing gets
    // nothing — the gateway doesn't pick a level on its behalf.
    if let Some(effort) = thinking_to_reasoning_effort(req) {
        out["reasoning_effort"] = json!(effort);
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
    // The effort level crosses; `enable_thinking` does not (see
    // `reasoning_effort_to_output_config`).
    if let Some(effort) = reasoning_effort_to_output_config(req) {
        out["output_config"] = json!({ "effort": effort });
    }
    out
}
