use std::io;

use super::McpError;
use super::McpFailure;
use super::cause::Cause;

/// A server that could not be connected: the error it is reported with and
/// the cause taken from what failed, before that became text.
#[derive(Debug)]
pub(super) struct Unconnected {
    pub(super) error: McpError,
    pub(super) cause: Cause,
}

impl Unconnected {
    /// `server`'s command could not be started, for `source`.
    pub(super) fn spawn(server: &str, source: io::Error) -> Self {
        Self {
            error: McpError::Spawn {
                server: server.to_string(),
                source,
            },
            cause: Cause::of(McpFailure::Spawn),
        }
    }

    /// `server` did not complete its handshake or its tool listing, for
    /// `cause`, which `message` words.
    pub(super) fn handshake(server: &str, message: String, cause: Cause) -> Self {
        Self {
            error: McpError::Handshake {
                server: server.to_string(),
                message,
            },
            cause,
        }
    }

    /// `server` could not be reached as configured, for the `failure` that
    /// `message` words.
    pub(super) fn transport(server: &str, message: &str, failure: McpFailure) -> Self {
        Self {
            error: McpError::Transport {
                server: server.to_string(),
                message: message.to_string(),
            },
            cause: Cause::of(failure),
        }
    }
}
