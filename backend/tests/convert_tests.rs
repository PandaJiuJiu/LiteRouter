//! Protocol conversion: Anthropic Messages ⇄ OpenAI ChatCompletions.
//!
//! These are pure functions over `serde_json::Value`, so the whole suite runs
//! without a database, a server, or a network. The value is that the mapping
//! tables are the kind of thing that breaks silently — a `tool_use` that stops
//! emitting `tool_calls`, a `finish_reason` that falls through to a default —
//! and none of that shows up until a real SDK chokes downstream.

use literouter::convert::*;
use serde_json::{json, Value};

fn v(s: &str) -> Value {
    serde_json::from_str(s).unwrap()
}

// ===================== anthropic request -> openai request =====================

#[test]
fn system_string_becomes_a_system_message() {
    let out = anthropic_req_to_openai(
        &v(r#"{"system":"be brief","messages":[{"role":"user","content":"hi"}]}"#),
        "gpt-4o",
    );
    assert_eq!(out["messages"][0], json!({"role":"system","content":"be brief"}));
    assert_eq!(out["messages"][1]["role"], "user");
    assert_eq!(out["model"], "gpt-4o");
}

#[test]
fn system_block_array_is_joined_with_newlines() {
    let out = anthropic_req_to_openai(
        &v(r#"{"system":[{"type":"text","text":"a"},{"type":"text","text":"b"}],
             "messages":[]}"#),
        "m",
    );
    assert_eq!(out["messages"][0]["content"], "a\nb");
}

#[test]
fn empty_system_is_dropped_entirely() {
    let out = anthropic_req_to_openai(&v(r#"{"system":"","messages":[]}"#), "m");
    assert_eq!(out["messages"], json!([]));
}

#[test]
fn max_tokens_defaults_to_4096_when_absent() {
    // Anthropic requires max_tokens; OpenAI would default it. The gateway has
    // to supply one or the upstream rejects the request.
    let out = anthropic_req_to_openai(&v(r#"{"messages":[]}"#), "m");
    assert_eq!(out["max_tokens"], 4096);
    let out = anthropic_req_to_openai(&v(r#"{"max_tokens":7,"messages":[]}"#), "m");
    assert_eq!(out["max_tokens"], 7);
}

#[test]
fn stop_sequences_becomes_stop() {
    let out = anthropic_req_to_openai(
        &v(r#"{"stop_sequences":["X","Y"],"messages":[]}"#),
        "m",
    );
    assert_eq!(out["stop"], json!(["X", "Y"]));
    let out = anthropic_req_to_openai(&v(r#"{"messages":[]}"#), "m");
    assert!(out.get("stop").is_none());
}

#[test]
fn scalar_content_becomes_a_plain_string() {
    let out = anthropic_req_to_openai(
        &v(r#"{"messages":[{"role":"user","content":"hello"}]}"#),
        "m",
    );
    assert_eq!(out["messages"][0]["content"], "hello");
}

#[test]
fn multi_block_content_stays_an_array() {
    let out = anthropic_req_to_openai(
        &v(r#"{"messages":[{"role":"user","content":[
             {"type":"text","text":"look"},
             {"type":"text","text":"at this"}]}]}"#),
        "m",
    );
    assert_eq!(out["messages"][0]["content"].as_array().unwrap().len(), 2);
}

#[test]
fn base64_image_becomes_a_data_url_part() {
    let out = anthropic_req_to_openai(
        &v(r#"{"messages":[{"role":"user","content":[
             {"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"QUJD"}}]}]}"#),
        "m",
    );
    assert_eq!(
        out["messages"][0]["content"][0]["image_url"]["url"],
        "data:image/jpeg;base64,QUJD"
    );
}

#[test]
fn url_image_is_passed_through_verbatim() {
    let out = anthropic_req_to_openai(
        &v(r#"{"messages":[{"role":"user","content":[
             {"type":"image","source":{"type":"url","url":"https://x/y.png"}}]}]}"#),
        "m",
    );
    assert_eq!(
        out["messages"][0]["content"][0]["image_url"]["url"],
        "https://x/y.png"
    );
}

#[test]
fn image_with_unknown_source_type_is_skipped() {
    let out = anthropic_req_to_openai(
        &v(r#"{"messages":[{"role":"user","content":[
             {"type":"image","source":{"type":"weird"}}]}]}"#),
        "m",
    );
    // No usable parts left -> the message contributes nothing at all.
    assert_eq!(out["messages"], json!([]));
}

#[test]
fn tool_use_becomes_assistant_tool_calls() {
    let out = anthropic_req_to_openai(
        &v(r#"{"messages":[{"role":"assistant","content":[
             {"type":"tool_use","id":"toolu_1","name":"get_weather","input":{"city":"Paris"}}]}]}"#),
        "m",
    );
    let msg = &out["messages"][0];
    assert_eq!(msg["role"], "assistant");
    assert_eq!(msg["tool_calls"][0]["id"], "toolu_1");
    assert_eq!(msg["tool_calls"][0]["type"], "function");
    assert_eq!(msg["tool_calls"][0]["function"]["name"], "get_weather");
    // Anthropic's `input` is an object; OpenAI's `arguments` is a JSON *string*.
    assert_eq!(msg["tool_calls"][0]["function"]["arguments"], r#"{"city":"Paris"}"#);
}

#[test]
fn tool_result_becomes_a_role_tool_message() {
    let out = anthropic_req_to_openai(
        &v(r#"{"messages":[{"role":"user","content":[
             {"type":"tool_result","tool_use_id":"toolu_1","content":"sunny"}]}]}"#),
        "m",
    );
    assert_eq!(out["messages"][0]["role"], "tool");
    assert_eq!(out["messages"][0]["tool_call_id"], "toolu_1");
    assert_eq!(out["messages"][0]["content"], "sunny");
}

#[test]
fn tool_result_block_array_is_flattened_to_text() {
    let out = anthropic_req_to_openai(
        &v(r#"{"messages":[{"role":"user","content":[
             {"type":"tool_result","tool_use_id":"t1","content":[
                {"type":"text","text":"line1"},{"type":"text","text":"line2"}]}]}]}"#),
        "m",
    );
    assert_eq!(out["messages"][0]["content"], "line1\nline2");
}

#[test]
fn a_pure_tool_result_message_emits_only_the_tool_turn() {
    let out = anthropic_req_to_openai(
        &v(r#"{"messages":[{"role":"user","content":[
             {"type":"tool_result","tool_use_id":"t1","content":"done"}]}]}"#),
        "m",
    );
    assert_eq!(out["messages"].as_array().unwrap().len(), 1);
    assert_eq!(out["messages"][0]["role"], "tool");
}

#[test]
fn missing_tool_use_id_becomes_null_not_a_missing_key() {
    let out = anthropic_req_to_openai(
        &v(r#"{"messages":[{"role":"user","content":[{"type":"tool_result","content":"x"}]}]}"#),
        "m",
    );
    assert_eq!(out["messages"][0]["tool_call_id"], Value::Null);
}

#[test]
fn tools_are_wrapped_as_openai_functions() {
    let out = anthropic_req_to_openai(
        &v(r#"{"messages":[],"tools":[{"name":"f","description":"d","input_schema":{"type":"object"}}]}"#),
        "m",
    );
    assert_eq!(out["tools"][0]["type"], "function");
    assert_eq!(out["tools"][0]["function"]["name"], "f");
    assert_eq!(out["tools"][0]["function"]["parameters"]["type"], "object");
}

#[test]
fn tool_without_input_schema_gets_an_object_schema() {
    let out = anthropic_req_to_openai(&v(r#"{"messages":[],"tools":[{"name":"f"}]}"#), "m");
    assert_eq!(out["tools"][0]["function"]["parameters"], json!({"type":"object"}));
}

#[test]
fn empty_tools_array_emits_no_tools_key() {
    let out = anthropic_req_to_openai(&v(r#"{"messages":[],"tools":[]}"#), "m");
    assert!(out.get("tools").is_none());
    assert!(out.get("tool_choice").is_none());
}

#[test]
fn anthropic_tool_choice_any_maps_to_required() {
    let out = anthropic_req_to_openai(
        &v(r#"{"messages":[],"tools":[{"name":"f"}],"tool_choice":"any"}"#),
        "m",
    );
    assert_eq!(out["tool_choice"], "required");
}

#[test]
fn anthropic_tool_choice_named_tool_maps_to_openai_named_function() {
    let out = anthropic_req_to_openai(
        &v(r#"{"messages":[],"tools":[{"name":"f"}],"tool_choice":{"type":"tool","name":"f"}}"#),
        "m",
    );
    assert_eq!(out["tool_choice"], json!({"type":"function","function":{"name":"f"}}));
}

#[test]
fn tool_choice_defaults_to_auto() {
    let out = anthropic_req_to_openai(&v(r#"{"messages":[],"tools":[{"name":"f"}]}"#), "m");
    assert_eq!(out["tool_choice"], "auto");
}

#[test]
fn missing_messages_field_keeps_the_system_message_only() {
    let out = anthropic_req_to_openai(&v(r#"{"system":"s"}"#), "m");
    assert_eq!(out["messages"], json!([{"role":"system","content":"s"}]));
}

// ===================== openai request -> anthropic request =====================

#[test]
fn system_message_is_lifted_out_of_the_message_list() {
    let out = openai_req_to_anthropic(
        &v(r#"{"messages":[{"role":"system","content":"be brief"},{"role":"user","content":"hi"}]}"#),
        "claude",
    );
    assert_eq!(out["system"], "be brief");
    // The system turn must not also survive as a user turn.
    assert_eq!(out["messages"].as_array().unwrap().len(), 1);
    assert_eq!(out["messages"][0]["role"], "user");
}

#[test]
fn first_system_message_wins_and_later_ones_are_dropped() {
    let out = openai_req_to_anthropic(
        &v(r#"{"messages":[{"role":"system","content":"one"},
             {"role":"system","content":"two"},{"role":"user","content":"x"}]}"#),
        "claude",
    );
    assert_eq!(out["system"], "one");
    assert_eq!(out["messages"].as_array().unwrap().len(), 1);
}

#[test]
fn assistant_tool_calls_become_tool_use_blocks() {
    let out = openai_req_to_anthropic(
        &v(r#"{"messages":[{"role":"assistant","content":null,"tool_calls":[
             {"id":"call_1","type":"function",
              "function":{"name":"f","arguments":"{\"a\":1}"}}]}]}"#),
        "claude",
    );
    let block = &out["messages"][0]["content"][0];
    assert_eq!(block["type"], "tool_use");
    assert_eq!(block["id"], "call_1");
    assert_eq!(block["name"], "f");
    // `arguments` is re-parsed back into a real object.
    assert_eq!(block["input"], json!({"a": 1}));
}

#[test]
fn unparseable_tool_arguments_become_an_empty_object() {
    // Better to forward `{}` than to abort the request — the upstream will
    // complain about a missing required arg, which is a clearer error than a
    // 500 from the gateway.
    let out = openai_req_to_anthropic(
        &v(r#"{"messages":[{"role":"assistant","tool_calls":[
             {"id":"c","function":{"name":"f","arguments":"not json"}}]}]}"#),
        "claude",
    );
    assert_eq!(out["messages"][0]["content"][0]["input"], json!({}));
}

#[test]
fn tool_role_message_becomes_a_user_turn_with_tool_result() {
    let out = openai_req_to_anthropic(
        &v(r#"{"messages":[{"role":"tool","tool_call_id":"call_1","content":"result text"}]}"#),
        "claude",
    );
    assert_eq!(out["messages"][0]["role"], "user");
    assert_eq!(out["messages"][0]["content"][0]["type"], "tool_result");
    assert_eq!(out["messages"][0]["content"][0]["tool_use_id"], "call_1");
    assert_eq!(out["messages"][0]["content"][0]["content"], "result text");
}

#[test]
fn data_url_image_url_becomes_a_base64_image_source() {
    let out = openai_req_to_anthropic(
        &v(r#"{"messages":[{"role":"user","content":[
             {"type":"image_url","image_url":{"url":"data:image/png;base64,QUJD"}}]}]}"#),
        "claude",
    );
    assert_eq!(
        out["messages"][0]["content"][0],
        json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"QUJD"}})
    );
}

#[test]
fn remote_image_url_stays_a_url_source() {
    let out = openai_req_to_anthropic(
        &v(r#"{"messages":[{"role":"user","content":[
             {"type":"image_url","image_url":{"url":"https://x/y.png"}}]}]}"#),
        "claude",
    );
    assert_eq!(
        out["messages"][0]["content"][0]["source"],
        json!({"type":"url","url":"https://x/y.png"})
    );
}

#[test]
fn openai_stop_becomes_anthropic_stop_sequences() {
    let out = openai_req_to_anthropic(&v(r#"{"messages":[],"stop":["X"]}"#), "claude");
    assert_eq!(out["stop_sequences"], json!(["X"]));
}

#[test]
fn openai_function_tools_become_anthropic_tools() {
    let out = openai_req_to_anthropic(
        &v(r#"{"messages":[],"tools":[{"type":"function",
             "function":{"name":"f","description":"d","parameters":{"type":"object"}}}]}"#),
        "claude",
    );
    assert_eq!(out["tools"][0]["name"], "f");
    assert_eq!(out["tools"][0]["input_schema"]["type"], "object");
}

#[test]
fn tool_entries_without_a_function_key_are_filtered_out() {
    let out = openai_req_to_anthropic(
        &v(r#"{"messages":[],"tools":[{"type":"function","function":{"name":"f"}},{"type":"other"}]}"#),
        "claude",
    );
    assert_eq!(out["tools"].as_array().unwrap().len(), 1);
}

#[test]
fn openai_required_tool_choice_becomes_anthropic_any() {
    let out = openai_req_to_anthropic(
        &v(r#"{"messages":[],"tools":[{"function":{"name":"f"}}],"tool_choice":"required"}"#),
        "claude",
    );
    assert_eq!(out["tool_choice"], json!({"type":"any"}));
}

#[test]
fn openai_named_function_choice_becomes_anthropic_named_tool() {
    let out = openai_req_to_anthropic(
        &v(r#"{"messages":[],"tools":[{"function":{"name":"f"}}],
             "tool_choice":{"type":"function","function":{"name":"g"}}}"#),
        "claude",
    );
    assert_eq!(out["tool_choice"], json!({"type":"tool","name":"g"}));
}

// ===================== responses =====================

#[test]
fn openai_response_becomes_an_anthropic_message() {
    let out = openai_resp_to_anthropic(
        &v(r#"{"id":"chatcmpl-abc","model":"gpt-4o",
             "choices":[{"index":0,"message":{"role":"assistant","content":"hello"},
                         "finish_reason":"stop"}],
             "usage":{"prompt_tokens":10,"completion_tokens":5}}"#),
        "claude",
    );
    assert_eq!(out["id"], "msg_abc"); // chatcmpl- prefix stripped
    assert_eq!(out["type"], "message");
    assert_eq!(out["content"][0], json!({"type":"text","text":"hello"}));
    assert_eq!(out["stop_reason"], "end_turn");
    assert_eq!(out["usage"]["input_tokens"], 10);
    assert_eq!(out["usage"]["output_tokens"], 5);
}

#[test]
fn openai_finish_reason_mapping_is_total() {
    let cases = [
        ("stop", "end_turn"),
        ("length", "max_tokens"),
        ("tool_calls", "tool_use"),
        ("function_call", "tool_use"),
        ("stop_sequence", "stop_sequence"),
        ("something_new_from_a_future_provider", "end_turn"),
    ];
    for (openai, anthropic) in cases {
        let out = openai_resp_to_anthropic(
            &json!({"choices":[{"message":{"content":"x"},"finish_reason":openai}]}),
            "m",
        );
        assert_eq!(out["stop_reason"], anthropic, "finish_reason {openai}");
    }
}

#[test]
fn openai_tool_calls_become_tool_use_blocks_in_the_response() {
    let out = openai_resp_to_anthropic(
        &v(r#"{"choices":[{"message":{"tool_calls":[
             {"id":"c1","function":{"name":"f","arguments":"{\"k\":2}"}}]},
             "finish_reason":"tool_calls"}]}"#),
        "claude",
    );
    assert_eq!(out["content"][0]["type"], "tool_use");
    assert_eq!(out["content"][0]["input"], json!({"k": 2}));
    assert_eq!(out["stop_reason"], "tool_use");
}

#[test]
fn empty_openai_content_produces_an_empty_content_array() {
    let out = openai_resp_to_anthropic(&v(r#"{"choices":[{"message":{"content":""}}]}"#), "m");
    assert_eq!(out["content"], json!([]));
    assert_eq!(out["stop_reason"], "end_turn");
}

#[test]
fn openai_response_without_choices_does_not_panic() {
    let out = openai_resp_to_anthropic(&json!({}), "m");
    assert_eq!(out["content"], json!([]));
    assert_eq!(out["usage"]["input_tokens"], 0);
    assert_eq!(out["id"], "msg_unknown");
}

#[test]
fn upstream_model_wins_over_the_requested_model_fallback() {
    let out = openai_resp_to_anthropic(&v(r#"{"model":"gpt-4o-mini","choices":[]}"#), "claude");
    assert_eq!(out["model"], "gpt-4o-mini");
    let out = openai_resp_to_anthropic(&v(r#"{"choices":[]}"#), "claude");
    assert_eq!(out["model"], "claude");
}

#[test]
fn anthropic_response_becomes_an_openai_completion() {
    let out = anthropic_resp_to_openai(
        &v(r#"{"id":"msg_abc","model":"claude-x",
             "content":[{"type":"text","text":"he"},{"type":"text","text":"llo"}],
             "stop_reason":"end_turn",
             "usage":{"input_tokens":3,"output_tokens":4}}"#),
        "gpt-4o",
    );
    assert_eq!(out["id"], "chatcmpl-abc");
    assert_eq!(out["object"], "chat.completion");
    // Two text blocks concatenate — OpenAI has no multi-block content.
    assert_eq!(out["choices"][0]["message"]["content"], "hello");
    assert_eq!(out["choices"][0]["finish_reason"], "stop");
    assert_eq!(out["usage"]["prompt_tokens"], 3);
    assert_eq!(out["usage"]["completion_tokens"], 4);
    assert_eq!(out["usage"]["total_tokens"], 7);
}

#[test]
fn anthropic_tool_use_becomes_openai_tool_calls() {
    let out = anthropic_resp_to_openai(
        &v(r#"{"id":"msg_1","content":[{"type":"tool_use","id":"t1","name":"f","input":{"a":1}}],
             "stop_reason":"tool_use"}"#),
        "m",
    );
    let msg = &out["choices"][0]["message"];
    assert_eq!(msg["tool_calls"][0]["id"], "t1");
    assert_eq!(msg["tool_calls"][0]["type"], "function");
    assert_eq!(msg["tool_calls"][0]["function"]["arguments"], r#"{"a":1}"#);
    assert_eq!(out["choices"][0]["finish_reason"], "tool_calls");
}

#[test]
fn a_tool_call_only_response_reports_null_content_not_empty_string() {
    let out = anthropic_resp_to_openai(
        &v(r#"{"id":"msg_1","content":[{"type":"tool_use","id":"t","name":"f","input":{}}],
             "stop_reason":"tool_use"}"#),
        "m",
    );
    assert_eq!(out["choices"][0]["message"]["content"], Value::Null);
}

#[test]
fn anthropic_stop_reason_mapping_is_total() {
    let cases = [
        ("end_turn", "stop"),
        ("stop_sequence", "stop"),
        ("max_tokens", "length"),
        ("tool_use", "tool_calls"),
        ("refusal", "stop"),
    ];
    for (anthropic, openai) in cases {
        let out = anthropic_resp_to_openai(&json!({"content": [], "stop_reason": anthropic}), "m");
        assert_eq!(out["choices"][0]["finish_reason"], openai, "stop_reason {anthropic}");
    }
}

#[test]
fn anthropic_response_without_content_or_usage_does_not_panic() {
    let out = anthropic_resp_to_openai(&json!({}), "m");
    assert_eq!(out["choices"][0]["message"]["content"], "");
    assert_eq!(out["choices"][0]["finish_reason"], "stop");
    assert_eq!(out["usage"]["total_tokens"], 0);
}

// ===================== errors =====================

#[test]
fn openai_error_becomes_an_anthropic_error() {
    let out = openai_err_to_anthropic(&v(r#"{"error":{"type":"invalid_request_error","message":"bad"}}"#));
    assert_eq!(out["type"], "error");
    assert_eq!(out["error"]["type"], "invalid_request_error");
    assert_eq!(out["error"]["message"], "bad");
}

#[test]
fn anthropic_error_becomes_an_openai_error_with_a_code() {
    let out = anthropic_err_to_openai(&v(r#"{"error":{"type":"overloaded_error","message":"busy"}}"#));
    assert_eq!(out["error"]["message"], "busy");
    assert_eq!(out["error"]["type"], "overloaded_error");
    // OpenAI SDKs read `code`, so it must mirror the type.
    assert_eq!(out["error"]["code"], "overloaded_error");
}

#[test]
fn malformed_error_bodies_fall_back_to_defaults() {
    let a = openai_err_to_anthropic(&json!({}));
    assert_eq!(a["error"]["type"], "api_error");
    assert_eq!(a["error"]["message"], "upstream error");
    let b = anthropic_err_to_openai(&json!({}));
    assert_eq!(b["error"]["type"], "api_error");
    assert_eq!(b["error"]["message"], "upstream error");
}

// ===================== usage =====================

#[test]
fn usage_parses_the_openai_spelling() {
    let u = parse_usage_obj(&json!({"prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14}));
    assert_eq!((u.prompt, u.completion, u.total), (10, 4, 14));
}

#[test]
fn usage_parses_the_anthropic_spelling() {
    let u = parse_usage_obj(&json!({"input_tokens": 10, "output_tokens": 4}));
    assert_eq!((u.prompt, u.completion, u.total), (10, 4, 14));
}

#[test]
fn a_missing_total_is_recomputed_from_the_parts() {
    let u = parse_usage_obj(&json!({"prompt_tokens": 7, "completion_tokens": 3}));
    assert_eq!(u.total, 10);
}

#[test]
fn an_upstream_total_below_the_parts_is_never_trusted() {
    // A provider reporting total < prompt+completion would under-bill if taken
    // at face value, so the sum wins.
    let u = parse_usage_obj(&json!({"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 2}));
    assert_eq!(u.total, 10);
}

#[test]
fn reasoning_tokens_are_read_from_the_nested_details_object() {
    let u = parse_usage_obj(
        &json!({"prompt_tokens": 10, "completion_tokens": 8,
                "completion_tokens_details": {"reasoning_tokens": 6}}),
    );
    assert_eq!(u.reasoning, 6);
    // reasoning is a subset of completion, not an addition to it.
    assert_eq!(u.completion, 8);
    assert_eq!(u.total, 18);
}

#[test]
fn anthropic_cache_counters_are_picked_up() {
    let u = parse_usage_obj(
        &json!({"input_tokens": 100, "output_tokens": 10,
                "cache_read_input_tokens": 40, "cache_creation_input_tokens": 15}),
    );
    assert_eq!(u.cache_read, 40);
    assert_eq!(u.cache_creation, 15);
}

#[test]
fn absent_usage_fields_read_zero_instead_of_panicking() {
    let u = parse_usage_obj(&json!({}));
    assert!(u.is_empty());
    let u = parse_usage_obj(&json!({"usage": null}));
    assert!(u.is_empty());
}

#[test]
fn sse_usage_is_read_from_the_openai_shape() {
    let u = usage_from_sse_payload(r#"{"usage":{"prompt_tokens":5,"completion_tokens":2}}"#).unwrap();
    assert_eq!((u.prompt, u.completion), (5, 2));
}

#[test]
fn sse_usage_is_read_from_message_start() {
    // Anthropic nests the input counts under `message.usage`.
    let u = usage_from_sse_payload(r#"{"type":"message_start","message":{"usage":{"input_tokens":9}}}"#)
        .unwrap();
    assert_eq!(u.prompt, 9);
}

#[test]
fn sse_usage_is_read_from_message_delta() {
    // ...and reports the final output count at the top level.
    let u = usage_from_sse_payload(r#"{"type":"message_delta","usage":{"output_tokens":7}}"#).unwrap();
    assert_eq!(u.completion, 7);
}

#[test]
fn sse_payload_without_usage_yields_none() {
    assert!(usage_from_sse_payload(r#"{"type":"content_block_delta"}"#).is_none());
    assert!(usage_from_sse_payload("not json").is_none());
}

#[test]
fn an_all_zero_usage_block_yields_none() {
    // Otherwise `usage()` would report Some(Usage::default()) and the log row
    // would claim a measured-but-zero spend.
    assert!(usage_from_sse_payload(r#"{"usage":{"prompt_tokens":0,"completion_tokens":0}}"#).is_none());
}
