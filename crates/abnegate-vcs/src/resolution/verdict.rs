use crate::resolution::ConflictSide;

/// What a repaired file is, judged against the conflicted file it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResolutionVerdict {
    /// Markers are gone and lines unique to each side survive. The only
    /// verdict that may be committed.
    Resolved,
    /// The conflicted file held no conflict hunks to judge against.
    NoConflict,
    /// The repair still holds conflict markers.
    MarkersRemain,
    /// The repair is blank where the conflicted file was not.
    Emptied,
    /// None of the lines unique to this side survived the repair.
    Discarded(ConflictSide),
}

impl ResolutionVerdict {
    /// Whether the repair may be committed.
    pub fn accepted(self) -> bool {
        self == ResolutionVerdict::Resolved
    }

    /// The verdict, as a stable identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            ResolutionVerdict::Resolved => "resolved",
            ResolutionVerdict::NoConflict => "no_conflict",
            ResolutionVerdict::MarkersRemain => "markers_remain",
            ResolutionVerdict::Emptied => "emptied",
            ResolutionVerdict::Discarded(ConflictSide::Ours) => "discarded_ours",
            ResolutionVerdict::Discarded(ConflictSide::Theirs) => "discarded_theirs",
        }
    }
}

impl std::fmt::Display for ResolutionVerdict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}
