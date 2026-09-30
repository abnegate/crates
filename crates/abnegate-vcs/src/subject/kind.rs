use std::fmt;

/// The kind of change a subject line announces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Kind {
    /// A new capability.
    Feat,
    /// A bug fix.
    Fix,
    /// A restructuring that changes no behaviour.
    Refactor,
    /// A speed or resource improvement.
    Perf,
    /// Tests only.
    Test,
    /// Documentation only.
    Docs,
    /// Formatting only.
    Style,
    /// Maintenance that fits no other kind.
    Chore,
}

impl Kind {
    /// Every kind, in the order a classifier is offered them.
    pub const ALL: &[Self] = &[
        Self::Feat,
        Self::Fix,
        Self::Refactor,
        Self::Perf,
        Self::Test,
        Self::Docs,
        Self::Style,
        Self::Chore,
    ];

    /// The kind a change falls back to when nothing classified it.
    ///
    /// It claims less than any other kind, so a wrong guess here understates
    /// the change rather than announcing one that was never made.
    pub const UNCLASSIFIED: Self = Self::Chore;

    /// The kind as a subject line spells it.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Feat => "feat",
            Self::Fix => "fix",
            Self::Refactor => "refactor",
            Self::Perf => "perf",
            Self::Test => "test",
            Self::Docs => "docs",
            Self::Style => "style",
            Self::Chore => "chore",
        }
    }

    /// The kind whose [`label`](Self::label) is `value`, ignoring case and
    /// surrounding whitespace, or `None`.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        Self::ALL.iter().copied().find(|kind| kind.label() == value)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}
