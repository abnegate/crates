use serde::Deserialize;

use crate::modality::vendor::rerank::hit::Hit;
use crate::modality::vendor::rerank::hit::UNSCORED;
use crate::provider::ProviderError;

const EMPTY: &str = "empty rerank response";

#[derive(Debug, Deserialize)]
pub(super) struct Response {
    results: Option<Vec<Hit>>,
}

impl Response {
    pub(super) fn into_scores(self, documents: usize) -> Result<Vec<f32>, ProviderError> {
        let hits = self.results.unwrap_or_default();
        if hits.is_empty() {
            return Err(ProviderError::parse(EMPTY));
        }

        let mut scores = vec![UNSCORED; documents];
        for hit in hits {
            if let Some(slot) = hit.index.and_then(|index| scores.get_mut(index)) {
                *slot = hit.score();
            }
        }
        Ok(scores)
    }
}
