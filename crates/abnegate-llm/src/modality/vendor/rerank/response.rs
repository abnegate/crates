use serde::Deserialize;

use crate::modality::vendor::rerank::hit::Hit;
use crate::modality::vendor::rerank::hit::UNSCORED;
use crate::provider::ProviderError;

const EMPTY: &str = "empty rerank response";
const NOTHING_SCORED: &str = "rerank response scored no document";

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
        let mut scored = false;
        for hit in hits {
            if let Some(slot) = hit.index.and_then(|index| scores.get_mut(index)) {
                let score = hit.score();
                scored |= score.is_some();
                *slot = score.unwrap_or(UNSCORED);
            }
        }
        if scored {
            Ok(scores)
        } else {
            Err(ProviderError::parse(NOTHING_SCORED))
        }
    }
}
