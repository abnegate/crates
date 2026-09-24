//! How one search ended, for a host's metrics.

use std::fmt;

/// How one search ended, as reported to a [`SearchObserver`](crate::SearchObserver).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Outcome {
    /// SearXNG answered and its results were read.
    Succeeded,
    /// Search is switched off, so no request was made.
    Disabled,
    /// Nothing was left of the message to search for, so no request was made.
    EmptyQuery,
    /// The request did not complete.
    Unreachable,
    /// SearXNG answered with a status other than success.
    Status,
    /// SearXNG answered with a body that is not its JSON.
    Malformed,
    /// SearXNG answered with more than this crate reads.
    TooLarge,
}

impl Outcome {
    /// A stable label, for a metrics dimension.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "ok",
            Self::Disabled => "disabled",
            Self::EmptyQuery => "empty_query",
            Self::Unreachable => "http_error",
            Self::Status => "status_error",
            Self::Malformed => "decode_error",
            Self::TooLarge => "too_large",
        }
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
