/// What became of one configured server the last time an
/// [`McpHub`](super::McpHub) connected it.
///
/// Only the hub produces one. A variant may gain a field in a minor release,
/// so a pattern outside this crate ends in `..`:
///
/// ```compile_fail,E0638
/// use abnegate_agent::mcp::McpOutcome;
///
/// fn failure(outcome: &McpOutcome) -> Option<&str> {
///     match outcome {
///         McpOutcome::Failed { error } => Some(error),
///         _ => None,
///     }
/// }
/// # let _ = failure;
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpOutcome {
    /// The server completed its handshake and listed its tools.
    #[non_exhaustive]
    Connected {
        /// The server's own name for each listed tool it
        /// [allows](super::McpServer::allows), in the order it listed them.
        tools: Vec<String>,
    },
    /// The server was not connected: it is not one the hub can start as
    /// written, or it failed to start or to answer in time.
    #[non_exhaustive]
    Failed {
        /// Why, in the hub's words.
        error: String,
    },
    /// The server is [disabled](super::McpServer::disabled), so it was not
    /// started.
    Disabled,
}
