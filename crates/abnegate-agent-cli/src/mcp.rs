//! Model Context Protocol servers attached to a Claude run.
//!
//! The servers are rendered into a private temporary file that the CLI reads
//! through `--mcp-config`, and only that file is loaded: `--strict-mcp-config`
//! keeps a repository's own `.mcp.json` from adding servers the caller never
//! chose. Values may hold `${VAR}` references, which the CLI expands itself
//! from the child's environment; the child is given each variable they name
//! from the host, and every literal value moves out of the file into a
//! variable of its own, so the file never holds a secret.
//!
//! [`McpConfig`] is the one configuration for every way a server reaches a
//! model: [`McpConfig::from_environment`] reads it under an application's own
//! prefix, a CLI is given [`McpConfig::render`]'s file, and a launcher of its
//! own, such as the MCP hub in `abnegate-agent`, starts each command server
//! with [`McpServer::environment_policy`]. A [disabled](McpServer::disabled)
//! server is left out of all of them.

mod attachment;
mod config;
mod config_error;
mod placeholders;
mod server;
mod transport;

pub use crate::mcp::attachment::McpAttachment;
pub use crate::mcp::config::DEFAULT_PREFIX;
pub use crate::mcp::config::McpConfig;
pub use crate::mcp::config_error::McpConfigError;
pub(crate) use crate::mcp::placeholders::expand;
pub use crate::mcp::server::McpServer;
pub use crate::mcp::transport::McpTransport;
