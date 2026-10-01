//! Kind of suggestion drawn from similar trials.

/// Kind of suggestion drawn from similar trials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum SuggestionKind {
    /// What worked on a similar attempt.
    Context,
    /// What already failed and should not be retried the same way.
    Avoid,
    /// A specific instruction taken from a successful lesson.
    Instruction,
    /// A failure mode that showed up more than once.
    Warning,
}

impl SuggestionKind {
    /// A stable label for digest keys.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Context => "context",
            Self::Avoid => "avoid",
            Self::Instruction => "instruction",
            Self::Warning => "warning",
        }
    }
}

impl std::fmt::Display for SuggestionKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}
