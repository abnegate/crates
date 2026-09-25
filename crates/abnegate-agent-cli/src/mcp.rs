//! Model Context Protocol servers attached to a Claude run.
//!
//! The servers are rendered into a private temporary file that the CLI reads
//! through `--mcp-config`, and only that file is loaded: `--strict-mcp-config`
//! keeps a repository's own `.mcp.json` from adding servers the caller never
//! chose.
//!
//! The file holds no literal environment or header value. Each environment
//! value, each run of literal text in a header, and each stdio command or
//! argument that refers to a variable moves into a generated variable of the
//! child's, named under a token drawn at random for every rendering: see
//! [`McpAttachment`]. A stdio server's `${VAR}` references are resolved here,
//! as the CLI would resolve them, and the child is given only the resolved
//! values, never the variables they name. A remote server's URL and the
//! references in its headers are written as they are, for the CLI alone to
//! expand under rules of its own that keep a credential from a server a
//! configuration names, and a remote server that refers to a generated
//! variable never attaches. A stdio command or argument that refers to
//! nothing is written as it is too, so a secret belongs in a reference
//! there, as in a URL.
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
mod entry;
mod mismatch;
mod placeholders;
mod segment;
mod server;
mod transport;

pub use crate::mcp::attachment::McpAttachment;
pub use crate::mcp::config::DEFAULT_PREFIX;
pub use crate::mcp::config::McpConfig;
pub use crate::mcp::config_error::McpConfigError;
pub(crate) use crate::mcp::placeholders::expand;
pub(crate) use crate::mcp::placeholders::references;
pub(crate) use crate::mcp::placeholders::whole_reference;
pub use crate::mcp::server::McpServer;
pub use crate::mcp::transport::McpTransport;
