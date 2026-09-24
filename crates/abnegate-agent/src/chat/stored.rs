use chrono::NaiveDateTime;
use serde_json::Value;
use uuid::Uuid;

/// A message as the conversation store holds it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct StoredMessage {
    /// Whether this message became the one the chat's automatic title is
    /// drawn from.
    pub title_claimed: bool,
    /// The message's identity.
    pub id: Uuid,
    /// The chat it belongs to.
    pub chat_id: Uuid,
    /// Who wrote it, as the store names roles.
    pub role: String,
    /// What it says.
    pub content: String,
    /// Whatever the caller stored beside it.
    pub metadata: Option<Value>,
    /// When the store wrote it, when the store keeps that.
    pub created_at: Option<NaiveDateTime>,
}

impl StoredMessage {
    /// Message `id` in `chat_id`, written by `role`, with no metadata and no
    /// time, and not the chat's title.
    pub fn new(
        id: Uuid,
        chat_id: Uuid,
        role: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            title_claimed: false,
            id,
            chat_id,
            role: role.into(),
            content: content.into(),
            metadata: None,
            created_at: None,
        }
    }

    /// The same message, with `metadata` stored beside it.
    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// The same message, written at `created_at`.
    pub fn with_created_at(mut self, created_at: NaiveDateTime) -> Self {
        self.created_at = Some(created_at);
        self
    }

    /// The same message, the chat's automatic title drawn from it or not.
    pub fn with_title_claimed(mut self, claimed: bool) -> Self {
        self.title_claimed = claimed;
        self
    }
}
