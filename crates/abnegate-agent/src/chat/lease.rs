use chrono::{DateTime, Utc};
use uuid::Uuid;

/// The right to respond in one chat, fenced so a stale holder cannot write.
#[derive(Debug, Clone)]
pub struct Lease {
    pub chat_id: Uuid,
    pub owner: Uuid,
    pub fence: i64,
    pub expires_at: DateTime<Utc>,
}
