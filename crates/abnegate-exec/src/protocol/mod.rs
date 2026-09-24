//! Protocol types and codec for driving an executor over a pipe.
//!
//! A runner is the process that executes commands; its client is whatever
//! started it and drives it over the pipe. This module provides the message
//! types they exchange and the NDJSON codec that frames them.

mod capability;
mod codec;
mod confinement_request;
mod error_code;
#[cfg(test)]
mod golden;
mod hello;
mod inbound_message;
mod log_level;
mod milliseconds;
mod outbound_message;
mod ping;
mod process_tree_request;
mod run_cancel;
mod run_start;
mod run_stdin;

pub use capability::Capability;
pub use codec::NdjsonCodec;
pub use confinement_request::ConfinementRequest;
pub use error_code::ErrorCode;
pub use hello::Hello;
pub use inbound_message::InboundMessage;
pub use log_level::LogLevel;
pub use outbound_message::OutboundMessage;
pub use ping::Ping;
pub use process_tree_request::ProcessTreeRequest;
pub use run_cancel::RunCancel;
pub use run_start::RunStart;
pub use run_stdin::RunStdin;
/// Protocol version for compatibility checking.
///
/// Confinement was added without a bump: `RunStart.confinement` defaults to
/// absent and the capability list is the negotiated extension point, so a
/// client that predates it keeps working unchanged. `ConfinementRequest`
/// grows the same way — `process_tree` defaults to absent, which is the
/// single-command confinement that shipped first.
pub const PROTOCOL_VERSION: &str = "1.0";
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_version_constant() {
        assert_eq!(PROTOCOL_VERSION, "1.0");
    }
}
