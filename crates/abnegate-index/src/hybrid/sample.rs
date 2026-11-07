//! Rows and the donor-trained ranker shared by the hybrid unit tests.

use crate::hybrid::result::HybridSearchResult;
use crate::rank::LinearRanker;

pub(super) fn trained_ranker() -> LinearRanker {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ranking/trained_ranker.rs"
    ))
}

pub(super) fn hit(chunk: u128, uri: &str, text: &str, score: f32) -> HybridSearchResult<u128> {
    HybridSearchResult::new(
        chunk,
        text,
        uri,
        uri.rsplit('/').next().unwrap_or(uri),
        score,
    )
    .with_keyword_rank(chunk as usize)
    .with_keyword_score(score / 10.0)
}
