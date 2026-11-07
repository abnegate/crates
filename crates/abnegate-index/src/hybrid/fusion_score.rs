//! One chunk's running contributions from both legs during fusion.

use crate::hybrid::candidate::Candidate;
use crate::hybrid::config::HybridSearchConfig;
use crate::hybrid::result::HybridSearchResult;

pub(super) struct FusionScore<Id> {
    id: Id,
    chunk_text: String,
    item_uri: String,
    item_title: String,
    keyword_rank: Option<usize>,
    semantic_rank: Option<usize>,
    keyword_score: Option<f32>,
    semantic_score: Option<f32>,
    keyword_fusion: f32,
    semantic_fusion: f32,
}

impl<Id> FusionScore<Id> {
    pub(super) fn new(candidate: Candidate<Id>) -> Self {
        Self {
            id: candidate.id,
            chunk_text: candidate.chunk_text,
            item_uri: candidate.item_uri,
            item_title: candidate.item_title,
            keyword_rank: None,
            semantic_rank: None,
            keyword_score: None,
            semantic_score: None,
            keyword_fusion: 0.0,
            semantic_fusion: 0.0,
        }
    }

    pub(super) fn record_keyword(&mut self, rank: usize, score: f32, fusion_constant: f32) {
        self.keyword_rank = Some(rank);
        self.keyword_score = Some(score);
        self.keyword_fusion = contribution(rank, fusion_constant);
    }

    pub(super) fn record_semantic(&mut self, rank: usize, score: f32, fusion_constant: f32) {
        self.semantic_rank = Some(rank);
        self.semantic_score = Some(score);
        self.semantic_fusion = contribution(rank, fusion_constant);
    }

    pub(super) fn into_result(self, config: &HybridSearchConfig) -> HybridSearchResult<Id> {
        let score = config.semantic_weight * self.semantic_fusion
            + (1.0 - config.semantic_weight) * self.keyword_fusion;
        HybridSearchResult {
            id: self.id,
            chunk_text: self.chunk_text,
            item_uri: self.item_uri,
            item_title: self.item_title,
            score,
            fusion_score: score,
            keyword_rank: self.keyword_rank,
            semantic_rank: self.semantic_rank,
            keyword_score: self.keyword_score,
            semantic_score: self.semantic_score,
        }
    }
}

fn contribution(rank: usize, fusion_constant: f32) -> f32 {
    1.0 / (fusion_constant + rank as f32)
}
