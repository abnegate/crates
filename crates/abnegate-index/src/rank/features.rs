//! The feature vector the learned ranker scores.

use crate::query::rewrite_query;
use crate::rank::chunk::fixture_penalty;
use crate::rank::chunk::is_test_chunk;
use crate::rank::identifier::identifier_role;

/// How many features [`extract_features`] produces.
pub const FEATURE_COUNT: usize = 15;

/// One hit's features, in [`extract_features`] order.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RankFeatures {
    /// The feature values.
    pub values: [f32; FEATURE_COUNT],
}

impl RankFeatures {
    /// Features with these `values`.
    pub fn new(values: [f32; FEATURE_COUNT]) -> Self {
        Self { values }
    }
}

/// The ranker features of one hit for `query`.
///
/// In order: the clamped semantic score; the normalised keyword score; whether
/// both legs found the hit; the share of identifiers found in the text, the
/// URI and the title; whether the text contains the whole query; query-token
/// coverage of the text and of the URI; the fusion score scaled by 40 and
/// clamped; the inverse keyword and semantic ranks, `1 / (60 + rank)`; whether
/// the URI is a fixture; whether the chunk defines an identifier; and whether
/// it is a test.
pub fn extract_features(
    query: &str,
    uri: &str,
    title: &str,
    text: &str,
    semantic: Option<f32>,
    keyword: Option<f32>,
    keyword_rank: Option<usize>,
    semantic_rank: Option<usize>,
    fusion: f32,
) -> RankFeatures {
    let rewritten = rewrite_query(query);
    let lowercase_uri = uri.to_ascii_lowercase();
    let lowercase_title = title.to_ascii_lowercase();
    let lowercase_text = text.to_ascii_lowercase();
    let lowercase_query = query.to_ascii_lowercase();

    let mut identifier_text_hits = 0.0;
    let mut identifier_uri_hits = 0.0;
    let mut identifier_title_hits = 0.0;
    for identifier in &rewritten.identifiers {
        if text.contains(identifier) {
            identifier_text_hits += 1.0;
        }
        let lowercase_identifier = identifier.to_ascii_lowercase();
        if !lowercase_identifier.is_empty() && lowercase_uri.contains(&lowercase_identifier) {
            identifier_uri_hits += 1.0;
        }
        if !lowercase_identifier.is_empty() && lowercase_title.contains(&lowercase_identifier) {
            identifier_title_hits += 1.0;
        }
    }
    let identifier_count = rewritten.identifiers.len().max(1) as f32;
    let role = identifier_role(text, uri, &rewritten.identifiers);

    let tokens: Vec<&str> = query
        .split(|character: char| {
            !character.is_alphanumeric() && character != '_' && character != '-'
        })
        .filter(|token| token.len() >= 2)
        .collect();
    let covered = tokens
        .iter()
        .filter(|token| lowercase_text.contains(&token.to_ascii_lowercase()))
        .count();
    let coverage = if tokens.is_empty() {
        0.0
    } else {
        covered as f32 / tokens.len() as f32
    };

    let path_overlap = tokens
        .iter()
        .filter(|token| lowercase_uri.contains(&token.to_ascii_lowercase()))
        .count() as f32
        / tokens.len().max(1) as f32;

    RankFeatures {
        values: [
            semantic.unwrap_or(0.0).clamp(0.0, 1.0),
            keyword_normalised(keyword.unwrap_or(0.0)),
            if semantic.is_some() && keyword.is_some() {
                1.0
            } else {
                0.0
            },
            (identifier_text_hits / identifier_count).min(1.0),
            (identifier_uri_hits / identifier_count).min(1.0),
            (identifier_title_hits / identifier_count).min(1.0),
            if !lowercase_query.is_empty() && lowercase_text.contains(&lowercase_query) {
                1.0
            } else {
                0.0
            },
            coverage,
            path_overlap,
            (fusion * 40.0).clamp(0.0, 1.0),
            inverse_rank(keyword_rank),
            inverse_rank(semantic_rank),
            fixture_penalty(&lowercase_uri),
            if role.is_definition() { 1.0 } else { 0.0 },
            if is_test_chunk(uri, text) { 1.0 } else { 0.0 },
        ],
    }
}

fn keyword_normalised(score: f32) -> f32 {
    (1.0 - (-score * 4.0).exp()).clamp(0.0, 1.0)
}

fn inverse_rank(rank: Option<usize>) -> f32 {
    rank.map(|rank| 1.0 / (60.0 + rank as f32)).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_leg_zeroes_its_features() {
        let features = extract_features(
            "align_vector",
            "auth.ts",
            "auth.ts",
            "login form",
            Some(0.9),
            None,
            None,
            Some(1),
            0.012,
        );
        assert_eq!(features.values[1], 0.0);
        assert_eq!(features.values[2], 0.0);
        assert_eq!(features.values[10], 0.0);
        assert_eq!(features.values[11], 1.0 / 61.0);
    }

    #[test]
    fn new_keeps_the_values() {
        let values = [0.5; FEATURE_COUNT];
        assert_eq!(RankFeatures::new(values).values, values);
    }
}
