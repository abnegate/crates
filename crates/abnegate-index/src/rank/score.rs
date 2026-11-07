//! Scoring one retrieved passage.

use crate::query::rewrite_query;
use crate::rank::chunk::is_fixture_uri;
use crate::rank::chunk::is_outline_chunk;
use crate::rank::chunk::is_sql_migration;
use crate::rank::chunk::is_test_chunk;
use crate::rank::chunk::query_wants_schema;
use crate::rank::features::extract_features;
use crate::rank::identifier::identifier_hit_count;
use crate::rank::identifier::identifier_part_coverage;
use crate::rank::identifier::identifier_role;
use crate::rank::lexical::blend_rank;
use crate::rank::lexical::lexical_cross_score;
use crate::rank::linear::LinearRanker;
use crate::rank::role::IdentifierRole;

/// Score a retrieved passage with `ranker` plus the lexical cross-encoder.
///
/// The ranker's score over [`extract_features`](crate::extract_features) is
/// blended with [`lexical_cross_score`](crate::lexical_cross_score) by
/// [`blend_rank`](crate::blend_rank), then an editorial bonus rewards
/// definitions and full identifier coverage and penalises outlines,
/// fixtures, tests, and migrations the question does not ask about.
pub fn score_hit(
    ranker: &LinearRanker,
    query: &str,
    uri: &str,
    title: &str,
    text: &str,
    semantic: Option<f32>,
    keyword: Option<f32>,
    keyword_rank: Option<usize>,
    semantic_rank: Option<usize>,
    fusion: f32,
) -> f32 {
    let features = extract_features(
        query,
        uri,
        title,
        text,
        semantic,
        keyword,
        keyword_rank,
        semantic_rank,
        fusion,
    );
    let learned = ranker.score(&features);
    let cross_encoder = lexical_cross_score(query, uri, title, text);
    blend_rank(learned, cross_encoder) + editorial_bonus(query, uri, text)
}

fn editorial_bonus(query: &str, uri: &str, text: &str) -> f32 {
    let identifiers = rewrite_query(query).identifiers;
    let role = identifier_role(text, uri, &identifiers);
    let covered = identifier_hit_count(text, &identifiers);
    let fraction = if identifiers.is_empty() {
        1.0
    } else {
        covered as f32 / identifiers.len() as f32
    };
    let mut bonus = match role {
        IdentifierRole::Exported => 1.05,
        IdentifierRole::Defined => 0.7,
        IdentifierRole::Mention | IdentifierRole::None => 0.0,
    } * fraction;
    if identifiers.len() >= 2 && covered == identifiers.len() {
        bonus += 0.5;
    }
    bonus += 0.2 * identifier_part_coverage(text, &identifiers);
    if is_outline_chunk(text) {
        bonus -= 0.95;
    }
    if is_fixture_uri(uri) {
        bonus -= 1.1;
    }
    if is_test_chunk(uri, text) {
        bonus -= 0.8;
    }
    if is_sql_migration(uri) && !query_wants_schema(query) {
        bonus -= 0.7;
    }
    bonus
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rank::features::FEATURE_COUNT;

    #[test]
    fn a_zero_ranker_still_prefers_the_definition_over_a_fixture() {
        let ranker = LinearRanker::new([0.0; FEATURE_COUNT], 0.0);
        let query = "What does should_skip_blob do?";
        let definition = score_hit(
            &ranker,
            query,
            "github://acme/widgets/content/mod.rs@main",
            "mod.rs",
            "pub fn should_skip_blob(uri: &str) -> bool { true }",
            Some(0.55),
            Some(0.40),
            Some(1),
            Some(6),
            0.009,
        );
        let fixture = score_hit(
            &ranker,
            query,
            "github://acme/widgets/evals/retrieval.json@main",
            "retrieval.json",
            "should_skip_blob",
            Some(0.74),
            Some(0.20),
            Some(2),
            Some(3),
            0.013,
        );
        assert!(
            definition > fixture,
            "definition={definition} fixture={fixture}"
        );
    }

    #[test]
    fn the_ranker_score_enters_through_the_blend() {
        let query = "login";
        let uri = "src/auth.ts";
        let zero = LinearRanker::new([0.0; FEATURE_COUNT], 0.0);
        let biased = LinearRanker::new([0.0; FEATURE_COUNT], 1.0);
        let base = score_hit(
            &zero,
            query,
            uri,
            "auth.ts",
            "login form",
            None,
            None,
            None,
            None,
            0.0,
        );
        let shifted = score_hit(
            &biased,
            query,
            uri,
            "auth.ts",
            "login form",
            None,
            None,
            None,
            None,
            0.0,
        );
        let cross_encoder = lexical_cross_score(query, uri, "auth.ts", "login form");
        assert_eq!(
            base,
            blend_rank(0.0, cross_encoder) + editorial_bonus(query, uri, "login form")
        );
        assert_eq!(
            shifted,
            blend_rank(1.0, cross_encoder) + editorial_bonus(query, uri, "login form")
        );
    }
}
