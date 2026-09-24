use abnegate_llm::ProviderError;
use thiserror::Error;

use crate::context::ContextError;

/// Why a run stopped short of an answer.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RunError {
    /// The provider failed to answer a round.
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),
    /// The conversation could not be prepared to fit the model's context.
    #[error("Context error: {0}")]
    Context(#[from] ContextError),
    /// The turn spent
    /// [`AgentConfig::maximum_iterations`](super::AgentConfig::maximum_iterations)
    /// model rounds without an answer.
    #[error("Maximum iterations exceeded")]
    IterationLimit,
    /// The model answered with neither text nor a tool call too many rounds in
    /// a row.
    #[error("The model answered with nothing usable too many times in a row")]
    Empty,
}
