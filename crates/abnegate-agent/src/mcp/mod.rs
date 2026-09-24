//! An MCP client: stdio servers whose tools join a [`ToolRegistry`].
//!
//! [`McpHub::connect`] launches every enabled command server in an
//! [`McpConfig`], completes the handshake with each, and lists what it
//! advertises. [`register`] then adds each remote tool to a registry as
//! `server__tool`, so one server cannot answer for another's tools or for a
//! built-in. A server that fails to start or to answer in time is logged and
//! skipped, and so is a [disabled](McpServer::disabled) one or one reached by
//! URL, which only a CLI can attach.
//!
//! The configuration is `abnegate-agent-cli`'s, re-exported here, so one
//! `mcp.json` drives both this client and a coding agent CLI.
//! [`McpConfig::from_environment`] reads it under an application's own prefix
//! (all optional):
//!
//! - `{PREFIX}_MCP_ENABLED`: master switch, default on.
//! - `{PREFIX}_MCP_SERVERS`: an inline document, the `mcpServers` shape or a
//!   bare map of servers.
//! - `{PREFIX}_MCP_CONFIG`: path to a JSON file of the same shape, read when
//!   no inline JSON is set; `~/.{prefix}/mcp.json` is read when neither is.
//! - `{PREFIX}_MCP_AUTO_CONNECT`: whether [`McpConfig::fallback`] may attach
//!   the caller's fallback server when nothing is configured, default on.
//!
//! [`DEFAULT_PREFIX`] is the prefix for an application with none of its own.
//!
//! ```no_run
//! use abnegate_agent::McpConfig;
//! use abnegate_agent::McpServer;
//! use abnegate_agent::mcp::with_defaults_and_mcp;
//!
//! # async fn tools() {
//! let config = McpConfig::from_environment("ACME")
//!     .fallback("notes", McpServer::command("notes-server", ["mcp"]));
//! let registry = with_defaults_and_mcp(&config).await;
//! println!("{:?}", registry.names());
//! # }
//! ```
//!
//! Children are given [`McpServer::environment_policy`]: the
//! [`DEFAULT_ENVIRONMENT`](abnegate_exec::DEFAULT_ENVIRONMENT) names from this
//! process and [`McpServer::environment`], or the whole environment when
//! [`McpServer::inherit_environment`] is set. When `ABNEGATE_EXEC_PROXY_URL`
//! is set, the proxy variables it implies are then applied on top, so a
//! server cannot route around it. Configure only trusted executables.
//!
//! [`ToolRegistry`]: crate::tool::ToolRegistry

mod error;
mod format;
mod guidance;
mod hub;
mod name;
mod register;
mod session;
mod tool;

pub use abnegate_agent_cli::mcp::DEFAULT_PREFIX;
pub use abnegate_agent_cli::mcp::McpConfig;
pub use abnegate_agent_cli::mcp::McpConfigError;
pub use abnegate_agent_cli::mcp::McpServer;
pub use abnegate_agent_cli::mcp::McpTransport;
pub use error::McpError;
pub use format::format_call_result;
pub use guidance::Guidance;
pub use guidance::guidance;
pub use guidance::guidance_for_tools;
pub use hub::McpHub;
pub use name::MAXIMUM_TOOL_NAME_CHARACTERS;
pub use name::SEPARATOR;
pub use name::qualified_tool_name;
pub use name::unique_qualified_tool_name;
pub use register::register;
pub use register::with_defaults_and_mcp;
