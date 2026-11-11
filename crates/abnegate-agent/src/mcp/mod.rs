//! An MCP client: stdio and streamable HTTP servers whose tools join a
//! [`ToolRegistry`].
//!
//! [`McpHub::connect`] launches every enabled command server in an
//! [`McpConfig`], connects to every enabled `url` server over streamable
//! HTTP, completes the handshake with each, and lists what it advertises,
//! keeping only the tools the server [allows](McpServer::allows), as a CLI
//! allows them. [`register`] then adds each remote tool to a registry as
//! `server__tool`, so one server cannot answer for another's tools or for a
//! built-in. That is the name a CLI gives it, `mcp__server__tool`, without
//! the `mcp__`, so one `tools` list scopes both alike (see
//! [`qualified_tool_name`]). A server that fails to start, to connect or to
//! answer in time is logged and recorded as an [`McpOutcome`], and so is a
//! [disabled](McpServer::disabled) one, one that is not
//! [valid](McpServer::valid), or an [`sse`](McpTransport::Sse) one, which
//! this client does not speak. [`McpHub::reconnect`] and [`McpHub::remove`]
//! then change one server without touching the others. One a CLI would
//! refuse because it is not [nameable](McpServer::nameable) still starts,
//! with a warning, its name and the tools it names sanitized as a CLI names
//! them.
//!
//! A [failed](McpOutcome::Failed) outcome names its kind as an
//! [`McpFailure`] and carries the HTTP status of the answer that failed it,
//! so a host can say why a server failed without reading the failure's text
//! or showing what the server sent. The status is `Some` exactly when the
//! request that failed the connection got an answer that was not 2xx, from
//! the server or a proxy in front of it, and the client observed it. It is
//! `None` for a 2xx the client could not use, for a request that got no
//! answer, and for every failure before a request is sent. The client does
//! not observe the answer to the last attempt it makes after retrying a
//! request an HTTP/2 server refused, so that answer can leave `None` too,
//! and one such as a bare 401 or 403 then fails as
//! [`Protocol`](McpFailure::Protocol). An `https` server behind a proxy is
//! reached through a tunnel, and the client never sees the proxy's refusal
//! of that tunnel, a 407 included, as a response: it is
//! [`Unreachable`](McpFailure::Unreachable) with no status.
//!
//! ```
//! use abnegate_agent::mcp::McpFailure;
//! use abnegate_agent::mcp::McpOutcome;
//!
//! fn reason(outcome: &McpOutcome) -> Option<String> {
//!     let McpOutcome::Failed { failure, status, .. } = outcome else {
//!         return None;
//!     };
//!     let reason = match (*failure, *status) {
//!         (McpFailure::Unauthorized, _) => "it needs credentials".to_owned(),
//!         (McpFailure::Unreachable | McpFailure::Timeout, _) => {
//!             "it could not be reached".to_owned()
//!         }
//!         (_, Some(status)) => format!("it answered {status}"),
//!         _ => "it could not be connected".to_owned(),
//!     };
//!     Some(reason)
//! }
//! # let _ = reason;
//! ```
//!
//! A `url` server is sent the headers [`McpServer::endpoint`] resolves, as
//! an [`McpEndpoint`]: each `${VAR}` in its URL and headers resolves only to
//! a [secret](McpServer::secrets) bound to it, or to its default, never to
//! this process's environment. A link-local or cloud metadata address is
//! refused, including one a name resolves to when the server is reached
//! directly; behind a proxy, which resolves names itself, only a literal
//! metadata address is refused. Only a same-origin 307 or 308 redirect is
//! followed, and no failure's text holds the server's URL or a bound secret.
//!
//! [`McpHub::connect`] and [`with_defaults_and_mcp`] contact every enabled
//! `url` server, where releases before this one skipped them: a server that
//! must not be contacted is [disabled](McpServer::disabled), with
//! [`McpServer::disable`] or `"disabled": true`.
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
//! The same configuration attaches its servers to a coding agent CLI run by
//! `abnegate-agent-cli`'s `CliProvider`, which writes the file its CLI reads.
//! There too, each `${VAR}` in a remote server's URL and headers resolves
//! only to a [secret](McpServer::secrets) bound to it with
//! [`McpConfig::with_secret`] or [`McpServer::with_secret`], or to its
//! default, and never against the environment the CLI is given. Which
//! servers attach, and the tools each allows there, depend on the
//! [`AgentKind`], re-exported here:
//!
//! ```
//! use abnegate_agent::mcp::AgentKind;
//! use abnegate_agent::mcp::McpConfig;
//! use abnegate_agent::mcp::McpServer;
//!
//! let config = McpConfig::default()
//!     .with_server("notes", McpServer::command("notes-server", ["mcp"]));
//! assert_eq!(config.allowed_tools(AgentKind::Claude), ["mcp__notes"]);
//! assert!(config.allowed_tools(AgentKind::Codex).is_empty());
//! ```
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
//! Each server is started with the references in its command, arguments and
//! environment [expanded](McpServer::expanded) from the
//! [secrets](McpServer::secrets) bound to it, and then this process's
//! environment, as a CLI expands them.
//!
//! Children are given [`McpServer::environment_policy`]: the
//! [`DEFAULT_ENVIRONMENT`](abnegate_exec::DEFAULT_ENVIRONMENT) names from this
//! process and [`McpServer::environment`], or the whole environment when
//! [`McpServer::inherit_environment`] is set. When `ABNEGATE_EXEC_PROXY_URL`
//! is set, the proxy variables it implies are then applied on top, so a
//! server cannot route around it; [`McpHub::connect_through`] applies a proxy
//! the caller hands it there instead. Configure only trusted executables.
//! A `url` server is reached through that same proxy, and never one the
//! environment's `HTTP_PROXY` or the system names, with its `NO_PROXY` hosts
//! reached directly.
//!
//! [`ToolRegistry`]: crate::tool::ToolRegistry

mod cause;
mod error;
mod failure;
#[cfg(test)]
mod fixture;
mod format;
mod guidance;
mod hub;
mod name;
mod outcome;
mod posted;
#[cfg(test)]
mod recorder;
mod redirect;
mod register;
mod remote;
mod retry;
mod scrubber;
mod server_process;
mod session;
mod tool;
mod unconnected;

pub use abnegate_agent_cli::AgentKind;
pub use abnegate_agent_cli::mcp::DEFAULT_PREFIX;
pub use abnegate_agent_cli::mcp::McpConfig;
pub use abnegate_agent_cli::mcp::McpConfigError;
pub use abnegate_agent_cli::mcp::McpEndpoint;
pub use abnegate_agent_cli::mcp::McpServer;
pub use abnegate_agent_cli::mcp::McpTransport;
pub use error::McpError;
pub use failure::McpFailure;
pub use format::format_call_result;
pub use guidance::Guidance;
pub use guidance::guidance;
pub use guidance::guidance_for_tools;
pub use hub::McpHub;
pub use name::MAXIMUM_TOOL_NAME_CHARACTERS;
pub use name::SEPARATOR;
pub use name::qualified_tool_name;
pub(crate) use name::server_prefix;
pub use name::unique_qualified_tool_name;
pub use outcome::McpOutcome;
pub use register::register;
pub use register::with_defaults_and_mcp;
pub use register::with_defaults_and_mcp_through;
