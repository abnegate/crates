#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! The parts an LLM agent is built from: a tool registry the model acts
//! through, a ReAct loop that drives it, and the context, history and session
//! handling a long conversation needs.
//!
//! [`tool`] holds the [`Tool`] trait and a [`ToolRegistry`] of file, command
//! and background-job tools. Every tool declares its own [`tool::Tier`], so
//! batching and confirmation read a call's consequences from the tool itself.
//! File tools stay beneath the working directory, resolving each path once
//! against a descriptor for the root so a symlink swapped in after the check
//! is never followed. Commands run in a process group of their own, killed
//! whole when they finish or overrun, and see only the allowlisted
//! environment the [`ToolContext`] names.
//!
//! [`Agent`] runs the loop: ask the model, run the tools it calls, feed their
//! results back, until it answers. A call whose tier needs confirming runs
//! only once [`AgentCallback::approve`] allows it, which by default it does
//! not; the approver is handed a [`tool::Preview`] of what the call will do,
//! verbatim but for its escapes and flagged whenever part of it had to be
//! left out. Every call is held to its tool's own timeout, and stopped with
//! the run if the run is dropped. Each request goes through
//! [`context::prepare`], which folds consumed history into a checkpoint when
//! the model's context would overflow, without ever editing the history.
//!
//! [`chat`] is the storage boundary a multi-turn chat session needs, leased so
//! only one response is ever live per chat; [`session`] saves and reloads agent
//! runs; [`template`] renders `{{key}}` prompt templates.
//!
//! ```no_run
//! use abnegate_agent::{Agent, AgentConfig, NoOpCallback, ToolContext, ToolRegistry};
//! use abnegate_llm::{LlmClient, LlmConfig};
//!
//! # async fn example() -> Result<(), abnegate_agent::AgentError> {
//! let llm = LlmClient::new(LlmConfig::new("http://127.0.0.1:4000/v1", "qwen3", ""));
//! let agent = Agent::new(
//!     llm,
//!     ToolRegistry::with_defaults(),
//!     AgentConfig::default(),
//!     ToolContext::default(),
//! );
//!
//! // NoOpCallback approves nothing that needs confirming: the model can read,
//! // list and search, and any write or command it asks for is refused.
//! let state = agent.run("List the files in src.", &NoOpCallback).await?;
//! println!("{:?}", state.final_response);
//! # Ok(())
//! # }
//! ```
//!
//! # Coming from claudear
//!
//! The prompt helpers here replace claudear's, and differ from them on
//! purpose:
//!
//! - [`context::estimate`] counts four bytes a token rounded up, not down,
//!   and charges each message 8 tokens of framing rather than 20.
//! - [`TemplateRenderer::render`] renders a key the context does not hold as
//!   nothing, where claudear left `{{key}}` in the prompt;
//!   [`TemplateRenderer::render_strict`] refuses such a template instead.
//! - `{{#if key}}` is false for an empty string as well as a missing key, and
//!   keys may contain `-`.
//!
//! # Features
//!
//! - `mcp`: an MCP client that launches stdio servers and adds their tools to
//!   a [`ToolRegistry`], configured from the environment under an
//!   application's own prefix.

pub mod chat;
pub mod context;
mod error;
#[cfg(feature = "mcp")]
#[cfg_attr(docsrs, doc(cfg(feature = "mcp")))]
pub mod mcp;
mod run;
pub mod session;
pub mod template;
#[cfg(test)]
mod test_support;
pub mod tool;

pub use abnegate_config::Application;
pub use abnegate_config::ApplicationError;
pub use abnegate_config::DEFAULT_APPLICATION;

pub use crate::error::Error;
pub use crate::error::Result;
#[cfg(feature = "mcp")]
#[cfg_attr(docsrs, doc(cfg(feature = "mcp")))]
pub use crate::mcp::McpConfig;
#[cfg(feature = "mcp")]
#[cfg_attr(docsrs, doc(cfg(feature = "mcp")))]
pub use crate::mcp::McpHub;
#[cfg(feature = "mcp")]
#[cfg_attr(docsrs, doc(cfg(feature = "mcp")))]
pub use crate::mcp::McpServerSpec;
pub use crate::run::Agent;
pub use crate::run::AgentCallback;
pub use crate::run::AgentConfig;
pub use crate::run::AgentError;
pub use crate::run::AgentPhase;
pub use crate::run::AgentState;
pub use crate::run::AgentStep;
pub use crate::run::NoOpCallback;
pub use crate::run::ToolCallResult;
pub use crate::session::FileSessionStore;
pub use crate::session::Session;
pub use crate::session::SessionStore;
pub use crate::session::SessionSummary;
pub use crate::template::TemplateContext;
pub use crate::template::TemplateError;
pub use crate::template::TemplateRenderer;
pub use crate::tool::Tool;
pub use crate::tool::ToolContext;
pub use crate::tool::ToolError;
pub use crate::tool::ToolRegistry;
pub use crate::tool::ToolResult;
