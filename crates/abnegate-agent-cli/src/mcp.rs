//! Model Context Protocol servers attached to a Claude run.
//!
//! The servers are rendered into a private temporary file that the CLI reads
//! through `--mcp-config`, and only that file is loaded: `--strict-mcp-config`
//! keeps a repository's own `.mcp.json` from adding servers the caller never
//! chose. A run whose configuration holds an enabled server loads strictly
//! even when no server attaches, every one refused or the file impossible to
//! write, and then loads none.
//!
//! The file holds no literal environment or header value. Each environment
//! and header value, and each URL, stdio command or argument that refers to
//! a variable, moves into a generated variable of the child's, named under a
//! token drawn at random for every rendering, so a file left behind by a run
//! killed before it could clean up exposes no secret. A URL, command or
//! argument that refers to nothing is written as it is, so a secret belongs
//! in a reference there.
//!
//! A stdio server's `${VAR}` references are resolved here, as the CLI would
//! resolve them, against the [`secrets`](McpServer::secrets) bound to the
//! server first, and then what the child is given and this process's
//! environment, with the agent's sign-in variables, the credential's own and
//! Claude Code's OAuth refresh token read as set but empty there. The child
//! is given each resolved value under its
//! generated name: resolving hands it nothing under the name of the variable
//! a reference names. Claude Code starts every stdio server, as it does the
//! agent's own tools, with its whole environment, so each of them can read
//! every generated variable and everything else the child holds.
//!
//! A remote server's references are resolved here too, against the server's
//! own [`secrets`](McpServer::secrets) alone, so a remote server is sent a
//! secret bound to it, or a default, and never a variable of this process's
//! or one the child is handed by name. The resolved value reaches the CLI in
//! a generated variable like any other, so a bound secret is kept from every
//! other remote server, not from the agent's tools or its stdio servers. A
//! remote server whose reference has neither a secret nor a default, whose
//! value still holds `${` once resolved, or whose header name holds `${`
//! never attaches, with a warning that names only the server: see
//! [`McpConfig::attachable`].
//!
//! [`McpConfig`] is the one configuration for every way a server reaches a
//! model: [`McpConfig::from_environment`] reads it under an application's own
//! prefix, a [`CliProvider`](crate::CliProvider) renders it for its CLI's
//! run, and a launcher of its own, such as the MCP hub in `abnegate-agent`,
//! starts each command server with [`McpServer::environment_policy`]. A
//! [disabled](McpServer::disabled) server is left out of all of them.

mod attachment;
mod config;
mod config_error;
mod entry;
mod mismatch;
mod placeholders;
mod refusal;
mod segment;
mod server;
mod template;
mod transport;

pub(crate) use crate::mcp::attachment::McpAttachment;
pub use crate::mcp::config::DEFAULT_PREFIX;
pub use crate::mcp::config::McpConfig;
pub use crate::mcp::config_error::McpConfigError;
pub(crate) use crate::mcp::placeholders::expand;
pub(crate) use crate::mcp::placeholders::references;
pub(crate) use crate::mcp::placeholders::whole_reference;
pub use crate::mcp::server::McpServer;
pub(crate) use crate::mcp::template::Template;
pub use crate::mcp::transport::McpTransport;
