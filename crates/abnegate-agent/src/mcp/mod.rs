//! An MCP client: stdio servers whose tools join a [`ToolRegistry`].
//!
//! [`McpHub::connect`] launches every configured server, completes the
//! handshake with each, and lists what it advertises. [`register`] then adds
//! each remote tool to a registry as `server__tool`, so one server cannot
//! answer for another's tools or for a built-in. A server
//! that fails to start or to answer in time is logged and skipped.
//!
//! [`McpConfig::with_prefix`] reads the configuration from the environment
//! under an application's prefix (all optional):
//!
//! - `{PREFIX}_MCP_ENABLED`: master switch, default on.
//! - `{PREFIX}_MCP_SERVERS`: inline JSON, the Cursor `mcpServers` shape or a
//!   bare map of server specs.
//! - `{PREFIX}_MCP_CONFIG`: path to a JSON file of the same shape, read when
//!   no inline JSON is set; `~/.{prefix}/mcp.json` is read when neither is.
//! - `{PREFIX}_MCP_AUTO_CONNECT`: whether [`McpConfig::fallback`] may attach
//!   the caller's fallback server when nothing is configured, default on.
//!
//! [`McpConfig::from_env`] does the same under [`DEFAULT_PREFIX`].
//!
//! Children are given the [`DEFAULT_ENVIRONMENT`](crate::tool::DEFAULT_ENVIRONMENT)
//! names from this process and [`McpServerSpec::environment`], or the whole
//! environment when [`McpServerSpec::inherit_environment`] is set. When
//! `ABNEGATE_EXEC_PROXY_URL` is set, the proxy variables it implies are then
//! applied on top, so a server cannot route around it. Configure only
//! trusted executables.
//!
//! [`ToolRegistry`]: crate::tool::ToolRegistry

mod config;
mod config_error;
mod error;
mod format;
mod guidance;
mod hub;
mod name;
mod register;
mod session;
mod spec;
mod tool;

pub use config::DEFAULT_PREFIX;
pub use config::McpConfig;
pub use config_error::McpConfigError;
pub use error::McpError;
pub use format::format_call_result;
pub use guidance::Guidance;
pub use guidance::guidance;
pub use guidance::guidance_for_tools;
pub use hub::McpHub;
pub use name::MAX_TOOL_NAME_CHARACTERS;
pub use name::SEPARATOR;
pub use name::qualified_tool_name;
pub use name::unique_qualified_tool_name;
pub use register::register;
pub use register::with_defaults_and_mcp;
pub use spec::McpServerSpec;
