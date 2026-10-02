//! Streaming protocol conversion — two SSE state machines.
//!
//! Both converters translate one event at a time and depend on having seen
//! earlier events (an OpenAI `tool_calls` fragment means nothing until the
//! fragment carrying the `id` started a block). A per-payload unit test
//! would therefore pass while real streams break, so every test here feeds a
//! **whole stream** through `on_data` and then `finish`, and asserts on the
//! ordered event sequence that comes out.
//!
//! Helper [`event_names`] pulls the `event:` lines out of the emitted SSE
//! blocks; [`datas`] pulls the JSON payloads.

use literouter::convert::*;
use serde_json::{json, Value};

/// The `event:` name of each emitted SSE block, in order.
fn event_names(events: &[String]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| {
            e.lines()
                .find(|l| l.starts_with("event: "))
                .map(|l| l.trim_start_matches("event: ").to_string())
        })
        .collect()
}

/// The JSON payload of each emitted SSE block, in order.
fn datas(events: &[String]) -> Vec<Value> {
    events
        .iter()
        .filter_map(|e| {
            e.lines()
                .find(|l| l.starts_with("data: "))
                .and_then(|l| serde_json::from_str(l.trim_start_matches("data: ")).ok())
        })
        .collect()
}

/// An OpenAI chunk carrying a text delta.
fn oai_text(text: &str) -> String {
    json!({"id":"chatcmpl-1","model":"gpt-4o",
           "choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]})
    .to_string()
}

// ===================== OpenAI stream -> Anthropic events =====================

#[test]
fn a_plain_text_stream_emits_a_well_formed_anthropic_sequence() {
    let mut c = OpenAiToAnthropicStream::new("claude-x");
    let mut events = c.on_data(&oai_text("Hello"));
    events.extend(c.on_data(&oai_text(" world")));
    events.extend(c.finish());

    assert_eq!(
        event_names(&events),
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ]
    );
    assert!(
        events.iter().all(|e| e.ends_with("\n\n")),
        "SSE blocks need a blank line terminator"
    );
}

#[test]
fn message_start_precedes_everything_and_carries_an_empty_content_array() {
    let mut c = OpenAiToAnthropicStream::new("claude-x");
    let events = c.on_data(&oai_text("hi"));
    let d = &datas(&events)[0];
    assert_eq!(d["type"], "message_start");
    assert_eq!(d["message"]["content"], json!([]));
    assert_eq!(d["message"]["model"], "claude-x");
    assert_eq!(d["message"]["role"], "assistant");
    assert_eq!(d["message"]["stop_reason"], Value::Null);
}

#[test]
fn message_start_is_emitted_exactly_once_per_stream() {
    let mut c = OpenAiToAnthropicStream::new("m");
    let mut events = c.on_data(&oai_text("a"));
    events.extend(c.on_data(&oai_text("b")));
    events.extend(c.on_data(&oai_text("c")));
    assert_eq!(
        event_names(&events)
            .iter()
            .filter(|n| *n == "message_start")
            .count(),
        1
    );
}

#[test]
fn text_deltas_all_target_the_same_block_index() {
    let mut c = OpenAiToAnthropicStream::new("m");
    let mut events = c.on_data(&oai_text("a"));
    events.extend(c.on_data(&oai_text("b")));
    let d = datas(&events);
    let starts: Vec<_> = d
        .iter()
        .filter(|x| x["type"] == "content_block_start")
        .collect();
    assert_eq!(starts.len(), 1, "one text block for one contiguous run");
    let deltas: Vec<_> = d
        .iter()
        .filter(|x| x["type"] == "content_block_delta")
        .collect();
    assert!(deltas.iter().all(|x| x["index"] == starts[0]["index"]));
    assert_eq!(deltas[0]["delta"]["type"], "text_delta");
}

#[test]
fn an_empty_text_delta_emits_nothing() {
    // Providers send a leading `{"role":"assistant"}` chunk with no content.
    let mut c = OpenAiToAnthropicStream::new("m");
    let events = c.on_data(
        &json!({"id":"chatcmpl-1","choices":[{"index":0,"delta":{"role":"assistant"}}]})
            .to_string(),
    );
    assert_eq!(event_names(&events), vec!["message_start"]);
}

#[test]
fn the_role_only_chunk_still_starts_the_message() {
    let mut c = OpenAiToAnthropicStream::new("m");
    let events = c.on_data(
        &json!({"id":"chatcmpl-1","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]})
            .to_string(),
    );
    assert_eq!(event_names(&events), vec!["message_start"]);
}

#[test]
fn finish_reason_is_translated_into_the_message_delta() {
    let mut c = OpenAiToAnthropicStream::new("m");
    c.on_data(&oai_text("hi"));
    c.on_data(&json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}).to_string());
    let events = c.finish();
    let d = datas(&events);
    let md = d.iter().find(|x| x["type"] == "message_delta").unwrap();
    assert_eq!(md["delta"]["stop_reason"], "tool_use");
    assert_eq!(md["delta"]["stop_sequence"], Value::Null);
}

#[test]
fn a_stream_cut_off_before_any_finish_reason_defaults_to_end_turn() {
    let mut c = OpenAiToAnthropicStream::new("m");
    c.on_data(&oai_text("hi"));
    let d = datas(&c.finish());
    let md = d.iter().find(|x| x["type"] == "message_delta").unwrap();
    assert_eq!(md["delta"]["stop_reason"], "end_turn");
}

#[test]
fn a_tool_call_stream_merges_fragments_into_one_block() {
    // This is the case a per-payload test misses: the id arrives in the first
    // fragment and the arguments stream in as continuation fragments, all
    // keyed by `index` rather than by id.
    let mut c = OpenAiToAnthropicStream::new("m");
    let mut events = c.on_data(
        &json!({
            "id":"chatcmpl-1",
            "choices":[{"index":0,"delta":{"tool_calls":[
                {"index":0,"id":"call_1","type":"function",
                 "function":{"name":"get_weather","arguments":""}}]}}]
        })
        .to_string(),
    );
    events.extend(
        c.on_data(
            &json!({
                "choices":[{"index":0,"delta":{"tool_calls":[
                    {"index":0,"function":{"arguments":"{\"city\":"}}]}}]
            })
            .to_string(),
        ),
    );
    events.extend(
        c.on_data(
            &json!({
                "choices":[{"index":0,"delta":{"tool_calls":[
                    {"index":0,"function":{"arguments":"\"Paris\"}"}}]}}]
            })
            .to_string(),
        ),
    );
    events.extend(c.finish());

    assert_eq!(
        event_names(&events),
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ]
    );
    let d = datas(&events);
    let start = d
        .iter()
        .find(|x| x["type"] == "content_block_start")
        .unwrap();
    assert_eq!(start["content_block"]["type"], "tool_use");
    assert_eq!(start["content_block"]["id"], "call_1");
    assert_eq!(start["content_block"]["name"], "get_weather");

    let deltas: Vec<_> = d
        .iter()
        .filter(|x| x["type"] == "content_block_delta")
        .collect();
    assert!(deltas
        .iter()
        .all(|x| x["delta"]["type"] == "input_json_delta"));
    let joined: String = deltas
        .iter()
        .map(|x| x["delta"]["partial_json"].as_str().unwrap())
        .collect();
    assert_eq!(joined, r#"{"city":"Paris"}"#);
}

#[test]
fn parallel_tool_calls_get_separate_blocks() {
    let mut c = OpenAiToAnthropicStream::new("m");
    c.on_data(
        &json!({"id":"c","choices":[{"delta":{"tool_calls":[
        {"index":0,"id":"a","function":{"name":"fa","arguments":""}},
        {"index":1,"id":"b","function":{"name":"fb","arguments":""}}]}}]})
        .to_string(),
    );
    let d = datas(&c.finish());
    let stops: Vec<_> = d
        .iter()
        .filter(|x| x["type"] == "content_block_stop")
        .collect();
    assert_eq!(stops.len(), 2);
    // Blocks close in ascending index order, never in completion order.
    assert!(stops[0]["index"].as_u64() < stops[1]["index"].as_u64());
}

#[test]
fn a_tool_fragment_arriving_before_any_id_still_opens_a_block() {
    let mut c = OpenAiToAnthropicStream::new("m");
    let events = c.on_data(
        &json!({"id":"c","choices":[{"delta":{"tool_calls":[
        {"index":0,"function":{"name":"f","arguments":"{}"}}]}}]})
        .to_string(),
    );
    let d = datas(&events);
    let start = d
        .iter()
        .find(|x| x["type"] == "content_block_start")
        .unwrap();
    assert_eq!(start["content_block"]["type"], "tool_use");
    // A synthesized id keeps the Anthropic wire format valid.
    assert!(start["content_block"]["id"]
        .as_str()
        .unwrap()
        .starts_with("toolu_"));
}

#[test]
fn text_after_a_tool_call_gets_its_own_later_block_index() {
    let mut c = OpenAiToAnthropicStream::new("m");
    let mut events = c.on_data(
        &json!({"id":"c","choices":[{"delta":{"tool_calls":[
        {"index":0,"id":"a","function":{"name":"f","arguments":""}}]}}]})
        .to_string(),
    );
    events.extend(c.on_data(&oai_text("after")));
    events.extend(c.finish());
    let d = datas(&events);
    let starts: Vec<_> = d
        .iter()
        .filter(|x| x["type"] == "content_block_start")
        .collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0]["content_block"]["type"], "tool_use");
    assert_eq!(starts[1]["content_block"]["type"], "text");
    assert_eq!(
        starts[0]["index"].as_u64().unwrap() + 1,
        starts[1]["index"].as_u64().unwrap()
    );
}

#[test]
fn usage_in_a_final_chunk_is_captured_and_reported() {
    let mut c = OpenAiToAnthropicStream::new("m");
    c.on_data(&oai_text("hi"));
    c.on_data(
        &json!({"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":5}}).to_string(),
    );
    let u = c.usage().expect("usage");
    assert_eq!((u.prompt, u.completion), (11, 5));
    // ...and surfaces in the closing message_delta.
    let d = datas(&c.finish());
    let md = d.iter().find(|x| x["type"] == "message_delta").unwrap();
    assert_eq!(md["usage"]["input_tokens"], 11);
    assert_eq!(md["usage"]["output_tokens"], 5);
}

#[test]
fn a_stream_without_usage_reports_none() {
    let mut c = OpenAiToAnthropicStream::new("m");
    c.on_data(&oai_text("hi"));
    assert!(c.usage().is_none());
}

#[test]
fn malformed_payloads_are_ignored_rather_than_fatal() {
    let mut c = OpenAiToAnthropicStream::new("m");
    assert!(c.on_data("{not json").is_empty());
    // Once the first valid chunk arrives the message has already started, so
    // a later bad chunk adds nothing.
    let before = c.on_data(&oai_text("a"));
    assert_eq!(
        event_names(&before),
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta"
        ]
    );
    assert!(c.on_data("}}}").is_empty());
}

#[test]
fn a_chunk_with_no_choices_still_emits_message_start() {
    // OpenAI sends a final usage-only chunk with `choices: []`.
    let mut c = OpenAiToAnthropicStream::new("m");
    let events = c.on_data(&json!({"id":"c","choices":[]}).to_string());
    assert_eq!(event_names(&events), vec!["message_start"]);
}

// ===================== Anthropic stream -> OpenAI chunks =====================

#[test]
fn a_plain_text_stream_emits_a_well_formed_openai_sequence() {
    let mut c = AnthropicToOpenAiStream::new("gpt-4o");
    let mut events = c.on_data(
        &json!({"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10}}})
            .to_string(),
    );
    events.extend(
        c.on_data(
            &json!({"type":"content_block_delta","index":0,
                "delta":{"type":"text_delta","text":"hi"}})
            .to_string(),
        ),
    );
    events.extend(c.finish());

    let names = event_names(&events);
    assert!(
        names.is_empty(),
        "openai chunks carry no event: lines: {names:?}"
    );

    let d = datas(&events);
    assert_eq!(d[0]["choices"][0]["delta"], json!({"role":"assistant"}));
    assert_eq!(d[0]["object"], "chat.completion.chunk");
    assert_eq!(d[1]["choices"][0]["delta"]["content"], "hi");
    assert_eq!(d[2]["choices"][0]["finish_reason"], "stop");
}

#[test]
fn the_stream_ends_with_a_usage_chunk_and_the_done_sentinel() {
    let mut c = AnthropicToOpenAiStream::new("m");
    c.on_data(
        &json!({"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":4}}})
            .to_string(),
    );
    c.on_data(
        &json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},
                      "usage":{"output_tokens":6}})
        .to_string(),
    );
    let events = c.finish();

    assert!(events.last().unwrap().starts_with("data: [DONE]"));
    let d = datas(&events);
    let usage_chunk = d.last().unwrap();
    assert_eq!(usage_chunk["choices"], json!([]));
    assert_eq!(usage_chunk["usage"]["prompt_tokens"], 4);
    assert_eq!(usage_chunk["usage"]["completion_tokens"], 6);
}

#[test]
fn input_and_output_counts_arrive_in_two_places_and_are_merged() {
    // message_start carries only the input counts; message_delta only the
    // final output count. Neither alone is enough.
    let mut c = AnthropicToOpenAiStream::new("m");
    c.on_data(
        &json!({"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":7}}})
            .to_string(),
    );
    c.on_data(&json!({"type":"message_delta","delta":{},"usage":{"output_tokens":3}}).to_string());
    let u = c.usage().expect("usage");
    assert_eq!((u.prompt, u.completion), (7, 3));
}

#[test]
fn message_delta_output_count_does_not_zero_the_input_count() {
    // The naive implementation assigns the parsed block wholesale, which would
    // reset `prompt` to 0 on every message_delta.
    let mut c = AnthropicToOpenAiStream::new("m");
    c.on_data(
        &json!({"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":9}}})
            .to_string(),
    );
    c.on_data(&json!({"type":"message_delta","usage":{"output_tokens":2}}).to_string());
    assert_eq!(c.usage().unwrap().prompt, 9);
}

#[test]
fn anthropic_cache_counters_survive_message_delta() {
    let mut c = AnthropicToOpenAiStream::new("m");
    c.on_data(
        &json!({"type":"message_start","message":{"id":"msg_1","usage":{
        "input_tokens":10,"cache_read_input_tokens":4,"cache_creation_input_tokens":2}}})
        .to_string(),
    );
    c.on_data(&json!({"type":"message_delta","usage":{"output_tokens":1}}).to_string());
    let u = c.usage().unwrap();
    assert_eq!((u.cache_read, u.cache_creation), (4, 2));
}

#[test]
fn a_tool_use_stream_emits_an_opening_chunk_then_argument_fragments() {
    let mut c = AnthropicToOpenAiStream::new("m");
    let mut events = c.on_data(
        &json!({"type":"content_block_start","index":0,"content_block":{
        "type":"tool_use","id":"toolu_1","name":"get_weather","input":{}}})
        .to_string(),
    );
    events.extend(
        c.on_data(
            &json!({"type":"content_block_delta","index":0,
        "delta":{"type":"input_json_delta","partial_json":"{\"city\":"}})
            .to_string(),
        ),
    );
    let d = datas(&events);

    assert_eq!(
        d[0]["choices"][0]["delta"]["tool_calls"][0]["id"],
        "toolu_1"
    );
    assert_eq!(d[0]["choices"][0]["delta"]["tool_calls"][0]["index"], 0);
    assert_eq!(
        d[0]["choices"][0]["delta"]["tool_calls"][0]["type"],
        "function"
    );
    assert_eq!(
        d[0]["choices"][0]["delta"]["tool_calls"][0]["function"]["name"],
        "get_weather"
    );
    // Fragment chunks carry no id — the client fills in the accumulator.
    assert!(d[1]["choices"][0]["delta"]["tool_calls"][0]
        .get("id")
        .is_none());
    // ...and they must point back at the block just opened.
    assert_eq!(d[1]["choices"][0]["delta"]["tool_calls"][0]["index"], 0);
    assert_eq!(
        d[1]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
        r#"{"city":"#
    );
}

#[test]
fn a_non_tool_content_block_start_is_not_reported_as_a_tool_call() {
    let mut c = AnthropicToOpenAiStream::new("m");
    let events = c.on_data(
        &json!({"type":"content_block_start","index":0,
                "content_block":{"type":"text","text":""}})
        .to_string(),
    );
    assert!(events.is_empty());
}

#[test]
fn the_second_tool_use_gets_index_one() {
    let mut c = AnthropicToOpenAiStream::new("m");
    c.on_data(
        &json!({"type":"content_block_start","content_block":{
        "type":"tool_use","id":"t0","name":"a","input":{}}})
        .to_string(),
    );
    let events = c.on_data(
        &json!({"type":"content_block_start","content_block":{
        "type":"tool_use","id":"t1","name":"b","input":{}}})
        .to_string(),
    );
    assert_eq!(
        datas(&events)[0]["choices"][0]["delta"]["tool_calls"][0]["index"],
        1
    );
}

#[test]
fn stop_reason_is_translated_into_the_final_chunk() {
    let mut c = AnthropicToOpenAiStream::new("m");
    c.on_data(&json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"}}).to_string());
    let d = datas(&c.finish());
    assert_eq!(d[0]["choices"][0]["finish_reason"], "length");
}

#[test]
fn a_truncated_stream_defaults_to_stop() {
    let mut c = AnthropicToOpenAiStream::new("m");
    c.on_data(
        &json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"x"}}).to_string(),
    );
    let d = datas(&c.finish());
    assert_eq!(d[0]["choices"][0]["finish_reason"], "stop");
}

#[test]
fn the_msg_prefix_is_stripped_from_the_chunk_id() {
    let mut c = AnthropicToOpenAiStream::new("m");
    let events = c.on_data(&json!({"type":"message_start","message":{"id":"msg_abc"}}).to_string());
    assert_eq!(datas(&events)[0]["id"], "chatcmpl-abc");
}

#[test]
fn unknown_event_types_are_ignored() {
    let mut c = AnthropicToOpenAiStream::new("m");
    assert!(c.on_data(&json!({"type":"ping"}).to_string()).is_empty());
    assert!(c.on_data("not json").is_empty());
    assert!(c.usage().is_none());
}

#[test]
fn content_block_stop_and_ping_produce_nothing() {
    let mut c = AnthropicToOpenAiStream::new("m");
    assert!(c
        .on_data(&json!({"type":"content_block_stop","index":0}).to_string())
        .is_empty());
    assert!(c
        .on_data(&json!({"type":"message_stop"}).to_string())
        .is_empty());
}

#[test]
fn finish_opens_a_message_when_no_payload_was_received() {
    // The bug we're fixing: an upstream that opened the stream and then
    // closed without ever sending a `data:` chunk left `started` false.
    // The old `finish()` ignored that and emitted `message_delta` +
    // `message_stop` with no `message_start` — malformed.
    let mut c = OpenAiToAnthropicStream::new("m");
    let events = c.finish();
    let joined = events.concat();
    assert!(
        joined.contains("\"type\":\"message_start\""),
        "finish() must open the message: {joined}"
    );
    // The closing envelope is also still produced.
    assert!(joined.contains("\"type\":\"message_stop\""));
}

#[test]
fn finish_does_not_re_open_a_message_that_was_already_opened() {
    let mut c = OpenAiToAnthropicStream::new("m");
    // Open the stream via a real payload, then close.
    let mut events = c.on_data(&json!({"id":"c1","choices":[{"delta":{}}]}).to_string());
    events.extend(c.finish());
    let joined = events.concat();
    // Exactly one message_start.
    assert_eq!(joined.matches("\"type\":\"message_start\"").count(), 1);
}
