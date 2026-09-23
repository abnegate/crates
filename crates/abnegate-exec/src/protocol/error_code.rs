use serde::Deserialize;
use serde::Serialize;

/// Error codes for structured error reporting
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    /// Protocol-level error (invalid message format)
    InvalidMessage,
    /// Job ID not found
    JobNotFound,
    /// Failed to spawn the process
    SpawnFailed,
    /// Command timed out
    Timeout,
    /// Output limit exceeded (output was truncated)
    OutputLimitExceeded,
    /// Job was cancelled
    Cancelled,
    /// Internal runner error
    InternalError,
    /// Workspace path is invalid
    InvalidWorkspace,
    /// Confinement was requested but this runner cannot prove it works
    ConfinementUnavailable,
}
