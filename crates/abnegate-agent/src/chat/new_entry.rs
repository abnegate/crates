use super::ReplayMessage;

/// A message to append to the conversation.
#[derive(Debug, Clone)]
pub struct NewEntry {
    pub id: String,
    pub message: ReplayMessage,
    /// Call ids whose tool may change external state. Recovery must never retry them.
    pub mutations: Vec<String>,
}
