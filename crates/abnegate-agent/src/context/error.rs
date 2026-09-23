use abnegate_llm::LlmError;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ContextError {
    #[error("Context capacity exceeded: {used} estimated input tokens; budget {budget}. {reason}")]
    Capacity {
        used: u64,
        budget: u64,
        reason: String,
    },
    #[error("Conversation checkpoint integrity error: {0}")]
    Integrity(String),
    #[error("Conversation summary failed: {0}")]
    Summary(String),
    #[error("Conversation summary transport failed: {0}")]
    Transport(#[from] LlmError),
}
