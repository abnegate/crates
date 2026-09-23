//! What a search can fail with.

use crate::outcome::Outcome;

/// SearXNG request error. A caller is expected to treat these as non-fatal.
///
/// No variant carries the request URL: it holds the query, and whatever
/// credential the configured template embeds.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SearchError {
    #[error("Search request failed: {0}")]
    Http(String),
    #[error("Search returned HTTP {0}")]
    Status(u16),
    #[error("Search returned a body that is not SearXNG JSON, at line {line} column {column}")]
    Malformed { line: usize, column: usize },
    #[error("Search returned more than {limit} bytes")]
    TooLarge { limit: usize },
}

impl SearchError {
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
        let malformed = SearchError::malformed(&error);
        assert_eq!(malformed.outcome(), Outcome::Malformed);
        assert_ne!(malformed.outcome(), Outcome::Unreachable);
    }
}
