// =========================== errors ===========================

use super::{json, Value};

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
