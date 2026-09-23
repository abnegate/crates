use abnegate_llm::LlmError;
use thiserror::Error;

use crate::context::ContextError;

/// Why a run stopped short of an answer.
#[derive(Debug, Error)]
pub enum AgentError {
    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),
    #[error("Context error: {0}")]
    Context(#[from] ContextError),
    #[error("Tool error: {0}")]
    Tool(String),
    #[error("Max iterations exceeded")]
    MaxIterations,
    #[error("Agent was cancelled")]
    Cancelled,
}
