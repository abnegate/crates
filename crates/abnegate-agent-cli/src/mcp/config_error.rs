use std::path::PathBuf;

use thiserror::Error;

/// Why an MCP configuration could not be read.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum McpConfigError {
    /// The document is none of the shapes
    /// [`McpConfig::from_value`](crate::mcp::McpConfig::from_value) reads.
    #[error("invalid MCP config: {0}")]
    Invalid(String),
    /// The entry for the server `name` does not describe a server.
    #[error("invalid MCP server '{name}': {source}")]
    Server {
        name: String,
        #[source]
        source: serde_json::Error,
    },
    /// The file at `path` could not be read.
    #[error("failed to read MCP config {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The text is not JSON.
    #[error("failed to parse MCP config: {0}")]
    Json(#[from] serde_json::Error),
}
