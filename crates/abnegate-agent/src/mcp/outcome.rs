use super::McpFailure;

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
///
/// A failure names its kind, and the status of the HTTP answer that failed
/// it when there was one, so a host can tell failures apart without reading
/// their text:
///
/// ```
/// use abnegate_agent::mcp::McpFailure;
/// use abnegate_agent::mcp::McpOutcome;
///
/// fn unauthorized(outcome: &McpOutcome) -> bool {
///     matches!(
///         outcome,
///         McpOutcome::Failed {
///             failure: McpFailure::Unauthorized,
///             status: Some(401),
///             ..
///         }
///     )
/// }
/// # let _ = unauthorized;
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
        /// The kind of failure, taken from what failed and never from
        /// `error`'s text.
        failure: McpFailure,
        /// The HTTP status of the answer, the server's or a proxy's in front
        /// of it, that failed the connection: `Some` exactly when the
        /// request that failed it received an answer whose status was not
        /// 2xx and the client observed it, and `None` otherwise, as for a
        /// failure before any request was sent. The client does not observe
        /// the answer to the last attempt it makes after retrying a request
        /// an HTTP/2 server refused, so that answer can leave `None`.
        status: Option<u16>,
    },
    /// The server is [disabled](super::McpServer::disabled), so it was not
    /// started.
    Disabled,
}
