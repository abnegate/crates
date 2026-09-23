//! A ReAct (reason + act) agent loop over a [`ToolRegistry`].
//!
//! [`Agent::run`] asks the model, runs the tools it calls, feeds their results
//! back, and repeats until the model answers or [`AgentConfig::max_iterations`]
//! is spent. Consecutive read-only calls run concurrently; anything that
//! mutates runs alone and in order, so a later read sees an earlier write.
//! Every request is prepared through [`context::prepare`], so a run that grows
//! past its model's context is compacted rather than truncated.
//!
//! [`ToolRegistry`]: crate::tools::ToolRegistry
//! [`context::prepare`]: crate::context::prepare

mod callback;
mod config;
mod error;
mod r#loop;
mod no_op;
mod phase;
mod result;
mod state;
mod step;

pub use callback::AgentCallback;
pub use config::AgentConfig;
pub use error::AgentError;
pub use r#loop::Agent;
pub use no_op::NoOpCallback;
pub use phase::AgentPhase;
pub use result::ToolCallResult;
pub use state::AgentState;
pub use step::AgentStep;
