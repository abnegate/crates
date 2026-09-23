#![deny(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! The parts an LLM agent is built from: a tool registry the model acts
//! through, a ReAct loop that drives it, and the context, history and session
//! handling a long conversation needs.
//!
//! [`tools`] holds the [`Tool`] trait and a [`ToolRegistry`] of file, command
//! and background-job tools. Every tool declares its own [`tools::Tier`], so
//! batching and confirmation read a call's consequences from the tool itself.
//! File tools stay beneath the working directory, resolving each path once
//! against a descriptor for the root so a symlink swapped in after the check
//! is never followed; commands run with only the environment the
//! [`ToolContext`] names.
//!
//! [`Agent`] runs the loop: ask the model, run the tools it calls, feed their
//! results back, until it answers. Each request goes through
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
//! let llm = LlmClient::new(LlmConfig {
//!     base_url: "http://127.0.0.1:4000/v1".to_string(),
//!     default_model: "qwen3".to_string(),
//!     ..LlmConfig::default()
//! });
//! let agent = Agent::new(
//!     llm,
//!     ToolRegistry::with_defaults(),
//!     AgentConfig::default(),
//!     ToolContext::default(),
//! );
//!
//! let state = agent.run("List the files in src.", &NoOpCallback).await?;
//! println!("{:?}", state.final_response);
//! # Ok(())
//! # }
//! ```
//!
//! # Features
//!
//! - `mcp`: an MCP client that launches stdio servers and adds their tools to
//!   a [`ToolRegistry`], configured from the environment under an
//!   application's own prefix.

pub mod agent;
pub mod chat;
pub mod context;
mod error;
#[cfg(feature = "mcp")]
#[cfg_attr(docsrs, doc(cfg(feature = "mcp")))]
pub mod mcp;
pub mod session;
pub mod template;
#[cfg(test)]
mod test_support;
pub mod tools;

pub use crate::agent::{
    Agent, AgentCallback, AgentConfig, AgentError, AgentPhase, AgentState, AgentStep, NoOpCallback,
    ToolCallResult,
};
pub use crate::error::{Error, Result};
#[cfg(feature = "mcp")]
#[cfg_attr(docsrs, doc(cfg(feature = "mcp")))]
pub use crate::mcp::{McpConfig, McpHub, McpServerSpec};
pub use crate::session::{FileSessionStore, Session, SessionStore, SessionSummary};
pub use crate::template::{TemplateContext, TemplateRenderer};
pub use crate::tools::{Tool, ToolContext, ToolError, ToolRegistry, ToolResult};
