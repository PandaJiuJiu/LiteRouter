// =========================== responses (non-streaming) ===========================

use super::{json, Value};
use crate::db;

pub(super) fn finish_openai_to_anthropic(reason: &str) -> &'static str {
    match reason {
        "stop" => "end_turn",
        "length" => "max_tokens",
        "tool_calls" | "function_call" => "tool_use",
        "stop_sequence" => "stop_sequence",
        _ => "end_turn",
    }
}

pub(super) fn finish_anthropic_to_openai(reason: &str) -> &'static str {
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
    let mut reasoning = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    for b in body
        .get("content")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
    {
        match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "text" => text.push_str(b.get("text").and_then(|t| t.as_str()).unwrap_or("")),
            // Anthropic's reasoning has no native OpenAI field; expose it the
            // way OpenAI-compatible chips do, so Anthropic->OpenAI clients
            // don't lose a reply that is entirely `thinking`.
            "thinking" => {
                reasoning.push_str(b.get("thinking").and_then(|t| t.as_str()).unwrap_or(""))
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
            _ => {}
        }
    }
    let stop_reason = body
        .get("stop_reason")
        .and_then(|r| r.as_str())
        .map(finish_anthropic_to_openai)
        .unwrap_or("stop");
    let mut message = if tool_calls.is_empty() {
        json!({ "role": "assistant", "content": text })
    } else {
        json!({ "role": "assistant", "content": if text.is_empty() { Value::Null } else { Value::String(text) }, "tool_calls": tool_calls })
    };
    if !reasoning.is_empty() {
        message["reasoning_content"] = Value::String(reasoning);
    }
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
        "created": db::now(),
        "model": body.get("model").and_then(|m| m.as_str()).unwrap_or(model),
        "choices": [{ "index": 0, "message": message, "finish_reason": stop_reason }],
        "usage": { "prompt_tokens": input, "completion_tokens": output, "total_tokens": input + output }
    })
}
