use std::path::PathBuf;

use thiserror::Error;

use crate::mcp::mismatch::Mismatch;

/// Why an MCP configuration could not be read.
///
/// A variant may gain a field in a minor release, so a pattern outside this
/// crate ends in `..`:
///
/// ```compile_fail,E0638
/// use abnegate_agent_cli::McpConfigError;
///
/// fn unreadable(error: &McpConfigError) -> Option<&std::path::Path> {
///     match error {
///         McpConfigError::Io { path, source: _ } => Some(path),
///         _ => None,
///     }
/// }
/// # let _ = unreadable;
/// ```
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum McpConfigError {
    /// The document is none of the shapes
    /// [`McpConfig::from_value`](crate::mcp::McpConfig::from_value) reads.
    #[error("invalid MCP config: {0}")]
    Invalid(String),
    /// The entry for the server `name` does not describe a server: `field`
    /// holds a value that is not `expected`, or the entry itself is not an
    /// object when `field` is `None`. Nothing from the document is quoted
    /// but the server's name, since a value in the wrong place may still be a
    /// secret.
    #[error("invalid MCP server '{name}': {}", Mismatch::new(*.field, .expected))]
    #[non_exhaustive]
    Server {
        /// The server's name in the document.
        name: String,
        /// The field, by its name on the wire, holding a value of the wrong
        /// type.
        field: Option<&'static str>,
        /// What the field, or the entry, must hold, as a JSON type.
        expected: &'static str,
    },
    /// The file at `path` could not be read.
    #[error("failed to read MCP config {path}: {source}")]
    #[non_exhaustive]
    Io {
        /// The file, as the caller named it.
        path: PathBuf,
        /// Why reading it failed.
        #[source]
        source: std::io::Error,
    },
    /// The text is not JSON.
    #[error("failed to parse MCP config: {0}")]
    Json(#[from] serde_json::Error),
}

impl McpConfigError {
    /// The server `name`'s entry, which `mismatch` says is not a server.
    pub(crate) fn server(name: &str, mismatch: Mismatch) -> Self {
        Self::Server {
            name: name.to_string(),
            field: mismatch.field,
            expected: mismatch.expected,
        }
    }
}
