//! Model Context Protocol servers attached to a Claude run.
//!
//! The servers are rendered into a private temporary file that the CLI reads
//! through `--mcp-config`, and only that file is loaded: `--strict-mcp-config`
//! keeps a repository's own `.mcp.json` from adding servers the caller never
//! chose. Values may hold `${VAR}` references, which the CLI expands itself,
//! so a secret can stay out of the file entirely.

mod config;
mod server;
mod transport;

pub use crate::mcp::config::McpConfig;
pub use crate::mcp::server::McpServer;
pub use crate::mcp::transport::McpTransport;
