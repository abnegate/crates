use serde_json::Value;
use uuid::Uuid;

/// A message as the conversation store holds it.
#[derive(Debug, Clone)]
pub struct StoredMessage {
    pub title_claimed: bool,
    pub id: Uuid,
    pub chat_id: Uuid,
    pub role: String,
    pub content: String,
    pub metadata: Option<Value>,
    pub created_at: Option<chrono::NaiveDateTime>,
}
