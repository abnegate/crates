use serde::Deserialize;

pub(super) const UNSCORED: f32 = 0.0;

#[derive(Debug, Deserialize)]
pub(super) struct Hit {
    pub(super) index: Option<usize>,
    relevance_score: Option<f32>,
    score: Option<f32>,
}

impl Hit {
    pub(super) fn score(&self) -> f32 {
        self.relevance_score.or(self.score).unwrap_or(UNSCORED)
    }
}
