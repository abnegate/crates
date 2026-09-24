//! What a search can fail with.

use crate::outcome::Outcome;

/// Why a search failed. A caller is expected to treat these as non-fatal.
///
/// No variant carries the request URL: it holds the query, and whatever
/// credential the configured template embeds.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Search is switched off in the [`WebSearchConfig`](crate::WebSearchConfig)
    /// the client was built from, so no request was made.
    #[error("Search is switched off")]
    Disabled,
    /// The request did not complete: the client could not be built, the
    /// instance could not be reached, or the answer stopped part way.
    #[error("Search request failed: {0}")]
    Http(String),
    /// The instance answered with this status rather than success. A redirect
    /// lands here too, since none is followed.
    #[error("Search returned HTTP {0}")]
    Status(u16),
    /// The answer is not SearXNG's JSON.
    #[error("Search returned a body that is not SearXNG JSON, at line {line} column {column}")]
    Malformed {
        /// Line of the answer at which decoding stopped, from 1.
        line: usize,
        /// Column of that line at which decoding stopped, from 1.
        column: usize,
    },
    /// The answer passed the most this crate reads.
    #[error("Search returned more than {limit} bytes")]
    TooLarge {
        /// The most bytes read from one answer.
        limit: usize,
    },
}

impl Error {
    /// A transport failure, with the request URL that `reqwest` appends removed.
    pub(crate) fn http(error: reqwest::Error) -> Self {
        Self::Http(error.without_url().to_string())
    }

    pub(crate) fn malformed(error: &serde_json::Error) -> Self {
        Self::Malformed {
            line: error.line(),
            column: error.column(),
        }
    }

    /// How this failure is reported to a [`SearchObserver`](crate::SearchObserver).
    pub fn outcome(&self) -> Outcome {
        match self {
            Self::Disabled => Outcome::Disabled,
            Self::Http(_) => Outcome::Unreachable,
            Self::Status(_) => Outcome::Status,
            Self::Malformed { .. } => Outcome::Malformed,
            Self::TooLarge { .. } => Outcome::TooLarge,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_that_is_not_json_is_its_own_outcome() {
        let error = serde_json::from_slice::<serde_json::Value>(b"<html>").expect_err("not JSON");
        let malformed = Error::malformed(&error);
        assert_eq!(malformed.outcome(), Outcome::Malformed);
        assert_ne!(malformed.outcome(), Outcome::Unreachable);
    }

    #[test]
    fn a_refusal_while_switched_off_is_its_own_outcome() {
        assert_eq!(Error::Disabled.outcome(), Outcome::Disabled);
        assert_eq!(Outcome::Disabled.as_str(), "disabled");
        assert_eq!(Error::Disabled.to_string(), "Search is switched off");
    }
}
