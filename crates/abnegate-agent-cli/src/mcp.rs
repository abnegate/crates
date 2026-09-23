//! Model Context Protocol servers attached to a Claude run.
//!
//! The servers are rendered into a private temporary file that the CLI reads
//! through `--mcp-config`, and only that file is loaded: `--strict-mcp-config`
//! keeps a repository's own `.mcp.json` from adding servers the caller never
//! chose. Values may hold `${VAR}` references, which the CLI expands itself
//! from the child's environment; the child is given each variable they name
//! from the host, and every literal value moves out of the file into a
//! variable of its own, so the file never holds a secret.

mod attachment;
mod config;
mod document;
mod placeholders;
mod server;
mod transport;

pub use crate::mcp::attachment::McpAttachment;
pub use crate::mcp::config::McpConfig;
pub use crate::mcp::server::McpServer;
pub use crate::mcp::transport::McpTransport;
