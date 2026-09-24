use thiserror::Error;

/// Why a conversation store refused or failed an operation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ChatError {
    /// Another response holds this chat's lease.
    #[error("This chat already has an active response")]
    Busy,
    /// The lease a write was made under expired or was taken over.
    #[error("Chat generation ownership expired or changed; the response was stopped")]
    LeaseLost,
    /// The checkpoint moved after it was read.
    #[error("Conversation checkpoint changed; retry from current history")]
    Conflict,
    /// The stored conversation contradicts itself, as the text says.
    #[error("Conversation integrity error: {0}")]
    Integrity(String),
    /// No evidence of that id belongs to this chat.
    #[error("Conversation evidence was not found in this chat")]
    NotFound,
    /// The storage behind the store failed, in its own words.
    #[error("conversation store failed: {0}")]
    Backend(String),
    /// A stored value could not be read or written as JSON.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// The HTTP client reading a deployment's context capacity failed.
    #[error("HTTP client failed: {0}")]
    Http(#[from] reqwest::Error),
}
