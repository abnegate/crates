use super::ReplayMessage;

/// A message to append to the conversation.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct NewEntry {
    /// The entry's identity in the conversation.
    pub id: String,
    /// The message to store.
    pub message: ReplayMessage,
    /// Call ids whose tool may change external state. Recovery must never retry them.
    pub mutations: Vec<String>,
}

impl NewEntry {
    /// `message`, to be stored as `id`, calling nothing that changes
    /// external state.
    pub fn new(id: impl Into<String>, message: ReplayMessage) -> Self {
        Self {
            id: id.into(),
            message,
            mutations: Vec::new(),
        }
    }

    /// The same entry, with the call ids whose tool may change external
    /// state.
    pub fn with_mutations(
        mut self,
        mutations: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.mutations = mutations.into_iter().map(Into::into).collect();
        self
    }
}
