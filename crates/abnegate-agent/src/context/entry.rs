use abnegate_llm::Message;

/// An immutable replay message plus request-local eligibility flags.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Entry {
    /// Names the entry in a summary's [`Coverage`](super::Coverage): unique
    /// and nonempty, and the same every time the entry is replayed.
    pub id: String,
    /// The message as it was sent or received.
    pub message: Message,
    /// Never compacted, such as the latest user message or trusted
    /// instructions.
    pub preserve: bool,
    /// The model has already been shown it, so it may be compacted.
    pub consumed: bool,
}

impl Entry {
    /// `message` under `id`, neither preserved nor yet consumed.
    pub fn new(id: impl Into<String>, message: Message) -> Self {
        Self {
            id: id.into(),
            message,
            preserve: false,
            consumed: false,
        }
    }

    /// The same entry, kept out of compaction when `preserve` is set.
    pub fn with_preserve(mut self, preserve: bool) -> Self {
        self.preserve = preserve;
        self
    }

    /// The same entry, marked as already shown to the model when `consumed`
    /// is set.
    pub fn with_consumed(mut self, consumed: bool) -> Self {
        self.consumed = consumed;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_entry_is_neither_preserved_nor_consumed_until_marked() {
        let entry = Entry::new("first", Message::user("hi"));
        assert_eq!(entry.id, "first");
        assert_eq!(entry.message.content.as_deref(), Some("hi"));
        assert!(!entry.preserve && !entry.consumed);

        let marked = entry.with_preserve(true).with_consumed(true);
        assert!(marked.preserve && marked.consumed);
    }
}
