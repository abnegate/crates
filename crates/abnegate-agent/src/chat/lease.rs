use chrono::DateTime;
use chrono::Utc;
use uuid::Uuid;

/// The right to respond in one chat, fenced so a stale holder cannot write.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Lease {
    /// The chat the lease is for.
    pub chat_id: Uuid,
    /// Who holds it.
    pub owner: Uuid,
    /// Rises with every new holder, so a write under an older lease is
    /// refused.
    pub fence: i64,
    /// When the lease lapses unless renewed.
    pub expires_at: DateTime<Utc>,
}

impl Lease {
    /// `owner`'s lease on `chat_id`, at `fence`, lapsing at `expires_at`.
    ///
    /// The arguments come in the fields' order, the chat before its holder:
    /// both are [`Uuid`]s, so the compiler cannot catch the two swapped.
    pub fn new(chat_id: Uuid, owner: Uuid, fence: i64, expires_at: DateTime<Utc>) -> Self {
        Self {
            chat_id,
            owner,
            fence,
            expires_at,
        }
    }
}
