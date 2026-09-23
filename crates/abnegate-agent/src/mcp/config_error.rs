use std::path::PathBuf;
use thiserror::Error;

/// Why an MCP configuration could not be read.
#[derive(Debug, Error)]
pub enum McpConfigError {
    #[error("invalid MCP config: {0}")]
    Invalid(String),
    #[error("failed to read MCP config {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse MCP config: {0}")]
    Json(#[from] serde_json::Error),
}
