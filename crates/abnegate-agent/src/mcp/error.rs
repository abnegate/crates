use thiserror::Error;

/// An MCP client failure. Connecting is best-effort: one bad server does not
/// take the others down.
#[derive(Debug, Error)]
pub enum McpError {
    #[error("failed to start MCP server '{server}': {source}")]
    Spawn {
        server: String,
        #[source]
        source: std::io::Error,
    },
    #[error("MCP handshake failed for '{server}': {message}")]
    Handshake { server: String, message: String },
    #[error("MCP tool call failed: {0}")]
    Call(String),
}
