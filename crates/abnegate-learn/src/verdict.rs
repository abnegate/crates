//! How one trial turned out.

/// How one trial turned out.
///
/// [`is_positive`](Self::is_positive) is success or partial progress.
/// [`is_negative`](Self::is_negative) is a failure, a skip, or an empty result
/// the next round should not blindly repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Verdict {
    /// The attempt produced the result it was after.
    Success,
    /// The attempt produced a useful signal short of success.
    Partial,
    /// The attempt ran and failed.
    Failure,
    /// The attempt could not run.
    Skip,
    /// The attempt ran and produced nothing.
    Empty,
}

impl Verdict {
    /// A stable label for storage and scoreboards.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Partial => "partial",
            Self::Failure => "failure",
            Self::Skip => "skip",
            Self::Empty => "empty",
        }
    }

    /// Success or partial progress.
    pub const fn is_positive(self) -> bool {
        matches!(self, Self::Success | Self::Partial)
    }

    /// A failure, skip, or empty result.
    pub const fn is_negative(self) -> bool {
        matches!(self, Self::Failure | Self::Skip | Self::Empty)
    }

    /// Parse a scoreboard label. Unknown text is [`Empty`](Self::Empty).
    ///
    /// `concrete` and `interesting` are accepted as [`Success`](Self::Success)
    /// and [`Partial`](Self::Partial) so a host that already stores those
    /// labels can hydrate without a second mapping table.
    pub fn parse(label: &str) -> Self {
        match label {
            "success" | "concrete" | "ok" => Self::Success,
            "partial" | "interesting" => Self::Partial,
            "failure" | "fail" | "failed" => Self::Failure,
            "skip" | "skipped" => Self::Skip,
            _ => Self::Empty,
        }
    }
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_round_trip_through_parse() {
        for verdict in [
            Verdict::Success,
            Verdict::Partial,
            Verdict::Failure,
            Verdict::Skip,
            Verdict::Empty,
        ] {
            assert_eq!(Verdict::parse(verdict.as_str()), verdict);
        }
        assert_eq!(Verdict::parse("concrete"), Verdict::Success);
        assert_eq!(Verdict::parse("interesting"), Verdict::Partial);
        assert_eq!(Verdict::parse("unknown"), Verdict::Empty);
        assert!(Verdict::Success.is_positive());
        assert!(Verdict::Skip.is_negative());
        assert!(!Verdict::Empty.is_positive());
    }
}
