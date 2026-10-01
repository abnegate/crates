//! One suggestion drawn from similar trials.

use crate::kind::SuggestionKind;

/// One suggestion drawn from similar trials.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Suggestion {
    /// Kind of suggestion.
    pub kind: SuggestionKind,
    /// Text to stamp onto the next attempt or inject into a prompt.
    pub text: String,
    /// Cosine score of the neighbour this came from, in `-1.0..=1.0`.
    pub confidence: f64,
    /// Trial ids the suggestion is based on.
    pub based_on: Vec<i64>,
}

impl Suggestion {
    /// A suggestion of `kind` with `text`, `confidence` held to `-1.0..=1.0`,
    /// and the trial ids in `based_on`.
    pub fn new(
        kind: SuggestionKind,
        text: impl Into<String>,
        confidence: f64,
        based_on: Vec<i64>,
    ) -> Self {
        Self {
            kind,
            text: text.into(),
            confidence: confidence.clamp(-1.0, 1.0),
            based_on,
        }
    }
}
