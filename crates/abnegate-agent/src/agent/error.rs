use abnegate_llm::LlmError;
use thiserror::Error;

use crate::context::ContextError;

/// Why a run stopped short of an answer.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AgentError {
    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),
    #[error("Context error: {0}")]
    Context(#[from] ContextError),
    #[error("Max iterations exceeded")]
    MaxIterations,
    #[error("The model answered with nothing usable too many times in a row")]
    Empty,
}
