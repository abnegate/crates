//! A past trial scored against a query embedding.

use crate::trial::Trial;

/// A past trial scored against a query embedding.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SimilarTrial {
    /// The neighbour.
    pub trial: Trial,
    /// Cosine similarity in `0.0..=1.0`.
    pub score: f64,
}

impl SimilarTrial {
    pub(crate) fn new(trial: Trial, score: f64) -> Self {
        Self { trial, score }
    }
}
