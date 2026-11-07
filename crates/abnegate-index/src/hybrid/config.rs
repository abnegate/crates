//! How the keyword and semantic legs are filtered and weighed.

use crate::query::rewrite_query;

const SEMANTIC_WEIGHT: f32 = 0.7;

const IDENTIFIER_SEMANTIC_WEIGHT: f32 = 0.45;

const FUSION_CONSTANT: f32 = 60.0;

const MINIMUM_KEYWORD_SCORE: f32 = 0.0;

const MINIMUM_SEMANTIC_SCORE: f32 = 0.35;

/// How the keyword and semantic legs of a hybrid search are filtered and
/// weighed in reciprocal rank fusion.
///
/// The default weighs the semantic leg 0.7, uses the fusion constant 60 from
/// the literature, sets no floor on keyword hits, and sets a floor of 0.35 on
/// the cosine similarity of semantic hits.
///
/// [`reciprocal_rank_fusion`](crate::reciprocal_rank_fusion) reads only the
/// weight and the constant. The two floors are for the host's own leg queries,
/// which filter before fusion.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct HybridSearchConfig {
    /// Weight of the semantic leg, from 0 to 1; the keyword leg weighs the
    /// rest.
    pub semantic_weight: f32,
    /// The constant `k` in each leg's `1 / (k + rank)` contribution.
    pub fusion_constant: f32,
    /// The keyword score a host's keyword leg should require. Nothing in this
    /// crate filters on it.
    pub minimum_keyword_score: f32,
    /// The similarity a host's semantic leg should require. Nothing in this
    /// crate filters on it.
    pub minimum_semantic_score: f32,
}

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self {
            semantic_weight: SEMANTIC_WEIGHT,
            fusion_constant: FUSION_CONSTANT,
            minimum_keyword_score: MINIMUM_KEYWORD_SCORE,
            minimum_semantic_score: MINIMUM_SEMANTIC_SCORE,
        }
    }
}

impl HybridSearchConfig {
    /// The default, with the legs balanced to a semantic weight of 0.45 when
    /// `query` names code identifiers.
    pub fn for_query(query: &str) -> Self {
        let config = Self::default();
        if rewrite_query(query).identifiers.is_empty() {
            config
        } else {
            config.with_semantic_weight(IDENTIFIER_SEMANTIC_WEIGHT)
        }
    }

    /// Set the weight of the semantic leg.
    pub fn with_semantic_weight(mut self, semantic_weight: f32) -> Self {
        self.semantic_weight = semantic_weight;
        self
    }

    /// Set the constant `k` in each leg's `1 / (k + rank)` contribution.
    pub fn with_fusion_constant(mut self, fusion_constant: f32) -> Self {
        self.fusion_constant = fusion_constant;
        self
    }

    /// Set the keyword score under which keyword hits are dropped.
    pub fn with_minimum_keyword_score(mut self, minimum_keyword_score: f32) -> Self {
        self.minimum_keyword_score = minimum_keyword_score;
        self
    }

    /// Set the similarity under which semantic hits are dropped.
    pub fn with_minimum_semantic_score(mut self, minimum_semantic_score: f32) -> Self {
        self.minimum_semantic_score = minimum_semantic_score;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_the_donor_weights() {
        let config = HybridSearchConfig::default();
        assert_eq!(config.semantic_weight, 0.7);
        assert_eq!(config.fusion_constant, 60.0);
        assert_eq!(config.minimum_keyword_score, 0.0);
        assert_eq!(config.minimum_semantic_score, 0.35);
    }

    #[test]
    fn builders_set_each_field() {
        let config = HybridSearchConfig::default()
            .with_semantic_weight(0.5)
            .with_fusion_constant(60.0)
            .with_minimum_keyword_score(0.1)
            .with_minimum_semantic_score(0.6);
        assert_eq!(config.semantic_weight, 0.5);
        assert_eq!(config.fusion_constant, 60.0);
        assert_eq!(config.minimum_keyword_score, 0.1);
        assert_eq!(config.minimum_semantic_score, 0.6);
    }

    #[test]
    fn identifier_questions_balance_the_legs() {
        assert_eq!(
            HybridSearchConfig::for_query("What does should_skip_blob do?").semantic_weight,
            0.45
        );
        assert_eq!(
            HybridSearchConfig::for_query("how are widgets restocked").semantic_weight,
            0.7
        );
    }
}
