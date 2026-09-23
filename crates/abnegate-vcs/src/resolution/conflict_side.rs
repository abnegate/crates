/// Which branch's work a repair dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictSide {
    Ours,
    Theirs,
}

impl ConflictSide {
    pub fn as_str(self) -> &'static str {
        match self {
            ConflictSide::Ours => "ours",
            ConflictSide::Theirs => "theirs",
        }
    }
}

impl std::fmt::Display for ConflictSide {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}
