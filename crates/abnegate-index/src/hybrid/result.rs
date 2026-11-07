//! A fused and ranked hybrid search hit.

/// A hybrid search hit: the chunk, where each leg ranked it, and its scores.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct HybridSearchResult<Id> {
    /// Identifies the chunk.
    pub id: Id,
    /// The chunk's text.
    pub chunk_text: String,
    /// The URI of the item the chunk belongs to.
    pub item_uri: String,
    /// The title of the item the chunk belongs to.
    pub item_title: String,
    /// The ranking score: the fusion score until the learned ranker and the
    /// lexical cross-encoder replace it.
    pub score: f32,
    /// The first-stage fusion score, kept for reporting after reranking.
    pub fusion_score: f32,
    /// The 1-based keyword rank, if the keyword leg found the chunk.
    pub keyword_rank: Option<usize>,
    /// The 1-based semantic rank, if the semantic leg found the chunk.
    pub semantic_rank: Option<usize>,
    /// The keyword leg's full-text rank, if it found the chunk.
    pub keyword_score: Option<f32>,
    /// The semantic leg's cosine similarity, if it found the chunk.
    pub semantic_score: Option<f32>,
}

impl<Id> HybridSearchResult<Id> {
    /// A hit on chunk `id` of the item at `item_uri` whose score and fusion
    /// score are both `score`, with no leg ranks or leg scores.
    pub fn new(
        id: Id,
        chunk_text: impl Into<String>,
        item_uri: impl Into<String>,
        item_title: impl Into<String>,
        score: f32,
    ) -> Self {
        Self {
            id,
            chunk_text: chunk_text.into(),
            item_uri: item_uri.into(),
            item_title: item_title.into(),
            score,
            fusion_score: score,
            keyword_rank: None,
            semantic_rank: None,
            keyword_score: None,
            semantic_score: None,
        }
    }

    /// Set the ranking score.
    pub fn with_score(mut self, score: f32) -> Self {
        self.score = score;
        self
    }

    /// Set the first-stage fusion score.
    pub fn with_fusion_score(mut self, fusion_score: f32) -> Self {
        self.fusion_score = fusion_score;
        self
    }

    /// Set the 1-based keyword rank.
    pub fn with_keyword_rank(mut self, keyword_rank: usize) -> Self {
        self.keyword_rank = Some(keyword_rank);
        self
    }

    /// Set the 1-based semantic rank.
    pub fn with_semantic_rank(mut self, semantic_rank: usize) -> Self {
        self.semantic_rank = Some(semantic_rank);
        self
    }

    /// Set the keyword leg's full-text rank.
    pub fn with_keyword_score(mut self, keyword_score: f32) -> Self {
        self.keyword_score = Some(keyword_score);
        self
    }

    /// Set the semantic leg's cosine similarity.
    pub fn with_semantic_score(mut self, semantic_score: f32) -> Self {
        self.semantic_score = Some(semantic_score);
        self
    }
}
