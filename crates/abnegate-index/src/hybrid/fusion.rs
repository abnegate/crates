//! Reciprocal rank fusion of the keyword and semantic legs.

use std::collections::HashMap;
use std::hash::Hash;

use crate::hybrid::candidate::Candidate;
use crate::hybrid::config::HybridSearchConfig;
use crate::hybrid::finalize::sort_by_score;
use crate::hybrid::fusion_score::FusionScore;
use crate::hybrid::result::HybridSearchResult;

/// Fuse the `keyword` and `semantic` legs, each in its own rank order, by
/// weighted reciprocal rank fusion.
///
/// A chunk at 1-based rank `r` in a leg contributes `1 / (k + r)` from that
/// leg, where `k` is [`fusion_constant`](HybridSearchConfig::fusion_constant);
/// its score is the semantic contribution times
/// [`semantic_weight`](HybridSearchConfig::semantic_weight) plus the keyword
/// contribution times the rest. Fusion uses positions, not leg scores, so the
/// legs need no normalising.
///
/// A chunk in both legs, matched by id, keeps the keyword leg's text. Results
/// are sorted by descending score with NaN last; equal scores keep first-seen
/// order, keyword leg first.
pub fn reciprocal_rank_fusion<Id: Clone + Eq + Hash>(
    keyword: Vec<Candidate<Id>>,
    semantic: Vec<Candidate<Id>>,
    config: &HybridSearchConfig,
) -> Vec<HybridSearchResult<Id>> {
    let mut positions: HashMap<Id, usize> = HashMap::with_capacity(keyword.len() + semantic.len());
    let mut scores: Vec<FusionScore<Id>> = Vec::with_capacity(keyword.len() + semantic.len());
    for (index, candidate) in keyword.into_iter().enumerate() {
        let score = candidate.score;
        let entry = entry(&mut positions, &mut scores, candidate);
        entry.record_keyword(index + 1, score, config.fusion_constant);
    }
    for (index, candidate) in semantic.into_iter().enumerate() {
        let score = candidate.score;
        let entry = entry(&mut positions, &mut scores, candidate);
        entry.record_semantic(index + 1, score, config.fusion_constant);
    }
    let mut results: Vec<HybridSearchResult<Id>> = scores
        .into_iter()
        .map(|score| score.into_result(config))
        .collect();
    sort_by_score(&mut results);
    results
}

fn entry<'a, Id: Clone + Eq + Hash>(
    positions: &mut HashMap<Id, usize>,
    scores: &'a mut Vec<FusionScore<Id>>,
    candidate: Candidate<Id>,
) -> &'a mut FusionScore<Id> {
    let position = match positions.get(&candidate.id) {
        Some(&position) => position,
        None => {
            positions.insert(candidate.id.clone(), scores.len());
            scores.push(FusionScore::new(candidate));
            scores.len() - 1
        }
    };
    &mut scores[position]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: u32, text: &str, score: f32) -> Candidate<u32> {
        Candidate::new(id, text, format!("uri{id}"), format!("Title {id}"), score)
    }

    #[test]
    fn a_chunk_in_both_legs_ranks_first() {
        let config = HybridSearchConfig::default();
        let keyword = vec![candidate(1, "Result 1", 0.9), candidate(2, "Result 2", 0.8)];
        let semantic = vec![
            candidate(2, "Result 2", 0.95),
            candidate(3, "Result 3", 0.85),
        ];

        let results = reciprocal_rank_fusion(keyword, semantic, &config);

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].id, 2);
        assert!(results[0].keyword_rank.is_some());
        assert!(results[0].semantic_rank.is_some());
        let keyword_only = results.iter().find(|result| result.id == 1).unwrap();
        assert!(keyword_only.keyword_rank.is_some());
        assert!(keyword_only.semantic_rank.is_none());
        let semantic_only = results.iter().find(|result| result.id == 3).unwrap();
        assert!(semantic_only.keyword_rank.is_none());
        assert!(semantic_only.semantic_rank.is_some());
    }

    #[test]
    fn tied_fusion_scores_keep_keyword_first_first_seen_order() {
        let config = HybridSearchConfig::default().with_semantic_weight(0.5);
        let keyword = vec![candidate(1, "a", 0.9), candidate(2, "b", 0.8)];
        let semantic = vec![candidate(3, "c", 0.7), candidate(4, "d", 0.6)];

        let results = reciprocal_rank_fusion(keyword, semantic, &config);

        assert_eq!(results[0].score.to_bits(), results[1].score.to_bits());
        assert_eq!(results[2].score.to_bits(), results[3].score.to_bits());
        let ids: Vec<u32> = results.iter().map(|result| result.id).collect();
        assert_eq!(ids, [1, 3, 2, 4]);
    }

    #[test]
    fn a_shared_chunk_keeps_the_keyword_text_and_both_leg_scores() {
        let config = HybridSearchConfig::default();
        let keyword = vec![candidate(7, "keyword text", 0.4)];
        let semantic = vec![candidate(7, "semantic text", 0.6)];

        let results = reciprocal_rank_fusion(keyword, semantic, &config);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].chunk_text, "keyword text");
        assert_eq!(results[0].keyword_score, Some(0.4));
        assert_eq!(results[0].semantic_score, Some(0.6));
        let expected = 0.7 * (1.0 / 61.0f32) + (1.0 - 0.7f32) * (1.0 / 61.0f32);
        assert_eq!(results[0].score.to_bits(), expected.to_bits());
        assert_eq!(results[0].fusion_score.to_bits(), expected.to_bits());
    }
}
