use thiserror::Error;

/// Why a tool could not run, as opposed to a tool that ran and failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ToolError {
    /// The call's arguments do not fit the tool's schema, as the text says.
    #[error("Invalid parameters: {0}")]
    InvalidParameters(String),
    /// The tool could not carry the call out, as the text says.
    #[error("Execution failed: {0}")]
    Execution(String),
    /// The file system or a child process failed.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// Arguments or output could not be read or written as JSON.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// No tool is registered under this name.
    #[error("Tool not found: {0}")]
    NotFound(String),
}
