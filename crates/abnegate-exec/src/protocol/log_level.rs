use serde::Deserialize;
use serde::Serialize;

/// Log levels for structured logging
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum LogLevel {
    /// Diagnostic detail
    Debug,
    /// Routine progress
    Info,
    /// Something the client should know about, such as truncated output
    Warn,
    /// A failure
    Error,
}
