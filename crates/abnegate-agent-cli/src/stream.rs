//! The raw Anthropic Messages API streaming wire format.
//!
//! These are the `message_start`, `content_block_delta` and related events the
//! Messages API itself streams, one level below the CLI's own
//! `--output-format stream-json`, which [`crate::parser::claude`] reads.

mod api_content_block;
mod api_delta;
mod api_stream_event;

pub use crate::stream::api_content_block::ApiContentBlock;
pub use crate::stream::api_delta::ApiDelta;
pub use crate::stream::api_stream_event::ApiStreamEvent;
