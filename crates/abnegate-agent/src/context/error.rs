use abnegate_llm::ProviderError;
use thiserror::Error;

/// Why a conversation could not be prepared for its model.
///
/// Only this crate builds one. [`Capacity`](Self::Capacity) may gain a field
/// in a minor release, so a pattern outside this crate ends in `..`:
///
/// ```compile_fail,E0638
/// use abnegate_agent::context::ContextError;
///
/// fn overflow(error: &ContextError) -> Option<u64> {
///     match error {
///         ContextError::Capacity { used, budget, reason: _ } => Some(used - budget),
///         _ => None,
///     }
/// }
/// # let _ = overflow;
/// ```
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ContextError {
    /// What cannot be compacted still overflows the input budget.
    #[error("Context capacity exceeded: {used} estimated input tokens; budget {budget}. {reason}")]
    #[non_exhaustive]
    Capacity {
        /// Estimated input tokens the request would spend.
        used: u64,
        /// The input budget it had to fit.
        budget: u64,
        /// What kept it from fitting.
        reason: String,
    },
    /// The history or its checkpoint no longer agree, or never did.
    #[error("Conversation checkpoint integrity error: {0}")]
    Integrity(String),
    /// The summarizer's answer could not stand in for the history.
    #[error("Conversation summary failed: {0}")]
    Summary(String),
    /// The provider asked for a summary failed to answer.
    #[error("Conversation summary provider failed: {0}")]
    Provider(#[from] ProviderError),
}
