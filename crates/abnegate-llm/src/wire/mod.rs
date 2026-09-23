//! The OpenAI-compatible request and response bodies.
//!
//! Every type here is re-exported flat from the crate root, so a caller writes
//! `abnegate_llm::Message` rather than naming this module.

mod chat_request;
mod chat_response;
mod chat_stream_chunk;
mod choice;
mod content_part;
mod function_call;
mod function_definition;
mod generated_image;
mod image_url;
mod message;
mod role;
mod specific_function;
mod stream_choice;
mod stream_delta;
mod stream_function_call;
mod stream_tool_call;
mod tool_call;
mod tool_choice;
mod tool_definition;
mod tool_mode;
mod usage;

use serde::Deserialize;

pub use crate::wire::chat_request::ChatRequest;
pub use crate::wire::chat_response::ChatResponse;
pub use crate::wire::chat_stream_chunk::ChatStreamChunk;
pub use crate::wire::choice::Choice;
pub use crate::wire::content_part::ContentPart;
pub use crate::wire::function_call::{FunctionCall, wire_arguments};
pub use crate::wire::function_definition::FunctionDefinition;
pub use crate::wire::generated_image::GeneratedImage;
pub use crate::wire::image_url::ImageUrl;
pub use crate::wire::message::Message;
pub use crate::wire::role::Role;
pub use crate::wire::specific_function::SpecificFunction;
pub use crate::wire::stream_choice::StreamChoice;
pub use crate::wire::stream_delta::StreamDelta;
pub use crate::wire::stream_function_call::StreamFunctionCall;
pub use crate::wire::stream_tool_call::StreamToolCall;
pub use crate::wire::tool_call::ToolCall;
pub use crate::wire::tool_choice::ToolChoice;
pub use crate::wire::tool_definition::ToolDefinition;
pub use crate::wire::tool_mode::ToolMode;
pub use crate::wire::usage::Usage;

/// Reads an explicit `null` as the default rather than failing.
///
/// Providers send `"images": null` and `"thinking_blocks": null` for a turn
/// that produced neither, and a chunk that fails to deserialise aborts the
/// whole stream.
pub(crate) fn null_to_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}
