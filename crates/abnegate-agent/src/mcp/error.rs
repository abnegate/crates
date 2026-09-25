use thiserror::Error;

/// An MCP client failure. Connecting is best-effort: one bad server does not
/// take the others down.
///
/// A variant may gain a field in a minor release, so a pattern outside this
/// crate ends in `..`:
///
/// ```compile_fail,E0638
/// use abnegate_agent::mcp::McpError;
///
/// fn server(error: &McpError) -> Option<&str> {
///     match error {
///         McpError::Handshake { server, message: _ } => Some(server),
///         _ => None,
///     }
/// }
/// # let _ = server;
/// ```
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum McpError {
    /// The server's command could not be started.
    #[error("failed to start MCP server '{server}': {source}")]
    #[non_exhaustive]
    Spawn {
        /// The server's name in the configuration.
        server: String,
        /// Why starting it failed.
        #[source]
        source: std::io::Error,
    },
    /// The server started but did not complete the MCP handshake, or list
    /// its tools, in time.
    #[error("MCP handshake failed for '{server}': {message}")]
    #[non_exhaustive]
    Handshake {
        /// The server's name in the configuration.
        server: String,
        /// What went wrong, in the client's words.
        message: String,
    },
    /// A call to one of a connected server's tools failed.
    #[error("MCP tool call failed: {0}")]
    Call(String),
}
