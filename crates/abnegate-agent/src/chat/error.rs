use thiserror::Error;

/// Why a conversation store refused or failed an operation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("This chat already has an active response")]
    Busy,
    #[error("Chat generation ownership expired or changed; the response was stopped")]
    LeaseLost,
    #[error("Conversation checkpoint changed; retry from current history")]
    Conflict,
    #[error("Conversation integrity error: {0}")]
    Integrity(String),
    #[error("Conversation evidence was not found in this chat")]
    NotFound,
    #[error("conversation store failed: {0}")]
    Backend(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("HTTP client failed: {0}")]
    Http(#[from] reqwest::Error),
}
