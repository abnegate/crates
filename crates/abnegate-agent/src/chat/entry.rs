use super::ReplayMessage;

/// A stored message and whether the model has already been shown it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Entry {
    /// The entry's identity in the conversation.
    pub id: String,
    /// The message as the store keeps it.
    pub message: ReplayMessage,
    /// Whether the model has already been shown it.
    pub consumed: bool,
}

impl Entry {
    /// `message`, stored as `id` and not yet shown to the model.
    pub fn new(id: impl Into<String>, message: ReplayMessage) -> Self {
        Self {
            id: id.into(),
            message,
            consumed: false,
        }
    }

    /// The same entry, shown to the model or not.
    pub fn with_consumed(mut self, consumed: bool) -> Self {
        self.consumed = consumed;
        self
    }
}
