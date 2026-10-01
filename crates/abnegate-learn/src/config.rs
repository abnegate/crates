//! Similarity and suggestion limits.

const SIMILARITY_FLOOR: f64 = 0.1;
const SIMILAR_COUNT: usize = 5;
const SUGGESTION_COUNT: usize = 8;

/// Limits for similarity search and the digest it feeds.
///
/// Built with [`new`](Self::new) or [`Default`], then `with_*`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Config {
    /// Lowest cosine score that still counts as similar, in `-1.0..=1.0`.
    pub minimum_similarity: f64,
    /// Most similar trials kept for suggestions.
    pub similar_limit: usize,
    /// Most suggestions kept in a digest.
    pub suggestion_limit: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

impl Config {
    /// Default floor `0.1`, five similar trials, eight suggestions.
    pub fn new() -> Self {
        Self {
            minimum_similarity: SIMILARITY_FLOOR,
            similar_limit: SIMILAR_COUNT,
            suggestion_limit: SUGGESTION_COUNT,
        }
    }

    /// This config with `minimum_similarity` held to `-1.0..=1.0`.
    pub fn with_minimum_similarity(mut self, minimum_similarity: f64) -> Self {
        self.minimum_similarity = minimum_similarity.clamp(-1.0, 1.0);
        self
    }

    /// This config with `similar_limit`. Zero keeps no neighbours.
    pub fn with_similar_limit(mut self, similar_limit: usize) -> Self {
        self.similar_limit = similar_limit;
        self
    }

    /// This config with `suggestion_limit`. Zero keeps no suggestions.
    pub fn with_suggestion_limit(mut self, suggestion_limit: usize) -> Self {
        self.suggestion_limit = suggestion_limit;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_is_held_to_the_cosine_interval() {
        assert_eq!(
            Config::new()
                .with_minimum_similarity(1.5)
                .minimum_similarity,
            1.0
        );
        assert_eq!(
            Config::new()
                .with_minimum_similarity(-1.5)
                .minimum_similarity,
            -1.0
        );
        assert_eq!(Config::new().suggestion_limit, 8);
    }
}
