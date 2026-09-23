//! Protocol types and codec for driving an executor over a pipe.
//!
//! This module provides the message types and NDJSON codec used for
//! communication between the backend and the Rust runner.

mod codec;
mod messages;

pub use codec::NdjsonCodec;
pub use messages::{
    Capability, ConfinementRequest, ErrorCode, InboundMessage, LogLevel, OutboundMessage,
    PROTOCOL_VERSION, ProcessTreeRequest,
};
