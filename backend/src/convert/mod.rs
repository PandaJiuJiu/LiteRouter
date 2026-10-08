//! Bidirectional protocol conversion: Anthropic Messages ⇄ OpenAI
//! ChatCompletions. When a client speaks one protocol but the channel only
//! serves the other, the relay converts requests before forwarding and
//! converts responses (buffered and streaming) back to the client's shape.
//! Mapping tables follow one-api's anthropic adaptor
//! (relay/adaptor/anthropic/main.go), inverted where needed.

pub mod errors;
pub mod requests;
pub mod responses;
pub mod streaming;

use serde_json::{json, Value};

// Re-export everything public so callers can use `use crate::convert::*;`
// or `use literouter::convert::*;` (tests) without knowing the submodule layout.
pub use errors::{anthropic_err_to_openai, openai_err_to_anthropic};
pub use requests::{anthropic_req_to_openai, openai_req_to_anthropic};
pub use responses::{anthropic_resp_to_openai, openai_resp_to_anthropic};
pub use streaming::{
    parse_usage_obj, usage_from_sse_payload, AnthropicToOpenAiStream, OpenAiToAnthropicStream,
    SseConverter, Usage,
};

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
