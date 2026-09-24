use thiserror::Error;

/// Why a tool could not run, as opposed to a tool that ran and failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ToolError {
    #[error("Invalid parameters: {0}")]
    InvalidParameters(String),
    #[error("Execution failed: {0}")]
    Execution(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Tool not found: {0}")]
    NotFound(String),
}
