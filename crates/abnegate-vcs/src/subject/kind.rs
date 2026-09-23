use std::fmt;

/// The kind of change a subject line announces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Kind {
    Feat,
    Fix,
    Refactor,
    Perf,
    Test,
    Docs,
    Style,
    Chore,
}

impl Kind {
    /// Every kind, in the order a classifier is offered them.
    pub const ALL: [Self; 8] = [
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
    /// It claims the least of the eight, so a wrong guess here understates the
    /// change rather than announcing one that was never made.
    pub const UNCLASSIFIED: Self = Self::Chore;

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

    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|kind| kind.label() == value)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}
