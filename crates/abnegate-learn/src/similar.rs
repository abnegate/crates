//! A past trial scored against a query embedding.

use crate::trial::Trial;

/// A past trial scored against a query embedding.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct SimilarTrial {
    /// The neighbour.
    pub trial: Trial,
    /// Cosine similarity in `-1.0..=1.0`.
    pub score: f64,
}

impl SimilarTrial {
    /// A neighbour `trial` scored `score`.
    pub fn new(trial: Trial, score: f64) -> Self {
        Self {
            trial,
            score: score.clamp(-1.0, 1.0),
        }
    }
}
