//! Reranking fused results with the learned ranker and ordering them for
//! display.

use std::cmp::Ordering;
use std::hash::Hash;

use crate::hybrid::file::cap_per_file;
use crate::hybrid::result::HybridSearchResult;
use crate::query::rewrite_query;
use crate::rank::LinearRanker;
use crate::rank::chunk_rank_tier;
use crate::rank::identifier_part_coverage;
use crate::rank::identifier_role;
use crate::rank::is_fixture_uri;
use crate::rank::is_test_chunk;
use crate::rank::role_strength;
use crate::rank::score_hit;

const MAXIMUM_CHUNKS_PER_FILE: usize = 2;

const SAME_DIRECTORY_AFFINITY: f32 = 0.22;

const SAME_CRATE_AFFINITY: f32 = 0.12;

/// Rerank `results` for `query` with [`apply_local_rerank`], drop fixtures,
/// and keep at most `limit` rows with at most two chunks per file through
/// [`cap_per_file`].
///
/// # Panics
///
/// See [`apply_local_rerank`].
pub fn finalize_ranking<Id: Clone + Eq + Hash>(
    ranker: &LinearRanker,
    results: Vec<HybridSearchResult<Id>>,
    query: &str,
    limit: usize,
) -> Vec<HybridSearchResult<Id>> {
    let ranked = apply_local_rerank(ranker, query, results)
        .into_iter()
        .filter(|result| !is_fixture_uri(&result.item_uri))
        .collect();
    cap_per_file(ranked, MAXIMUM_CHUNKS_PER_FILE, limit, query)
}

/// Replace each score with [`score_hit`] under `ranker`, then order the
/// results for `query`.
///
/// A fusion score of exactly 0 first takes the incoming score. Non-definition
/// chunks near a definition of one of the question's identifiers gain 0.22
/// in the same directory or 0.12 in the same crate, and the results end sorted
/// by the keys [`prefer_definition_chunks`] uses.
///
/// # Panics
///
/// Unlike [`prefer_definition_chunks`], this keeps the standard library's sort
/// even when a score is NaN, so it can panic if the sort notices the comparison
/// is not a total order. That needs a NaN ranked score and more than 20 rows. A
/// ranked score comes from `semantic_score`, `keyword_score`, `fusion_score`,
/// `score` when `fusion_score` is 0, and the ranker's weights, so keeping those
/// free of NaN and infinity rules it out.
pub fn apply_local_rerank<Id>(
    ranker: &LinearRanker,
    query: &str,
    results: Vec<HybridSearchResult<Id>>,
) -> Vec<HybridSearchResult<Id>> {
    let mut results = rescored(ranker, query, results);
    sort_ranked_by_definition(query, &mut results);
    results
}

fn rescored<Id>(
    ranker: &LinearRanker,
    query: &str,
    mut results: Vec<HybridSearchResult<Id>>,
) -> Vec<HybridSearchResult<Id>> {
    for result in &mut results {
        if result.fusion_score == 0.0 {
            result.fusion_score = result.score;
        }
        result.score = score_hit(
            ranker,
            query,
            &result.item_uri,
            &result.item_title,
            &result.chunk_text,
            result.semantic_score,
            result.keyword_score,
            result.keyword_rank,
            result.semantic_rank,
            result.fusion_score,
        );
    }
    sort_by_score(&mut results);
    boost_implementation_neighbors(query, &mut results);
    sort_by_score(&mut results);
    results
}

/// Stable-sort `results` by chunk tier, then identifier role strength, then
/// identifier part coverage, then score, each descending.
///
/// Within a file this shows the definition before a mention or identifier bag,
/// so a one-row-per-file pass keeps the implementation.
///
/// A NaN score compares equal to every other score, so it leaves its row where
/// the sort finds it. The order of a list holding one is stable and repeatable
/// but not meaningful, and it never panics.
pub fn prefer_definition_chunks<Id>(query: &str, results: &mut [HybridSearchResult<Id>]) {
    let keys = definition_keys(query, results);
    let mut order: Vec<usize> = (0..results.len()).collect();
    let comparison_is_total = !results.iter().any(|result| result.score.is_nan())
        && !keys.iter().any(|(_, _, coverage)| coverage.is_nan());
    let compare = |left: &usize, right: &usize| compare_definitions(&keys, results, *left, *right);
    if comparison_is_total {
        order.sort_by(compare);
    } else {
        insertion_sort_by(&mut order, compare);
    }
    reorder(results, &order);
}

fn sort_ranked_by_definition<Id>(query: &str, results: &mut [HybridSearchResult<Id>]) {
    let keys = definition_keys(query, results);
    let mut order: Vec<usize> = (0..results.len()).collect();
    order.sort_by(|left, right| compare_definitions(&keys, results, *left, *right));
    reorder(results, &order);
}

fn definition_keys<Id>(query: &str, results: &[HybridSearchResult<Id>]) -> Vec<(u8, u8, f32)> {
    let identifiers = rewrite_query(query).identifiers;
    results
        .iter()
        .map(|result| {
            (
                chunk_rank_tier(query, &result.item_uri, &result.chunk_text),
                role_strength(&result.chunk_text, &result.item_uri, &identifiers),
                identifier_part_coverage(&result.chunk_text, &identifiers),
            )
        })
        .collect()
}

fn compare_definitions<Id>(
    keys: &[(u8, u8, f32)],
    results: &[HybridSearchResult<Id>],
    left: usize,
    right: usize,
) -> Ordering {
    let (left_tier, left_role, left_coverage) = keys[left];
    let (right_tier, right_role, right_coverage) = keys[right];
    right_tier
        .cmp(&left_tier)
        .then_with(|| right_role.cmp(&left_role))
        .then_with(|| {
            right_coverage
                .partial_cmp(&left_coverage)
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| {
            results[right]
                .score
                .partial_cmp(&results[left].score)
                .unwrap_or(Ordering::Equal)
        })
}

/// Stable-sort `results` by descending score, NaN last.
pub fn sort_by_score<Id>(results: &mut [HybridSearchResult<Id>]) {
    results.sort_by(
        |left, right| match (left.score.is_nan(), right.score.is_nan()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal),
        },
    );
}

fn insertion_sort_by<T>(items: &mut [T], compare: impl Fn(&T, &T) -> Ordering) {
    for sorted in 1..items.len() {
        let mut position = sorted;
        while position > 0 && compare(&items[sorted], &items[position - 1]) == Ordering::Less {
            position -= 1;
        }
        items[position..=sorted].rotate_right(1);
    }
}

fn reorder<T>(items: &mut [T], order: &[usize]) {
    let mut destinations = vec![0; order.len()];
    for (destination, &source) in order.iter().enumerate() {
        destinations[source] = destination;
    }
    for index in 0..items.len() {
        while destinations[index] != index {
            let destination = destinations[index];
            items.swap(index, destination);
            destinations.swap(index, destination);
        }
    }
}

fn boost_implementation_neighbors<Id>(query: &str, results: &mut [HybridSearchResult<Id>]) {
    let identifiers = rewrite_query(query).identifiers;
    if identifiers.is_empty() {
        return;
    }
    let definitions: Vec<bool> = results
        .iter()
        .map(|result| {
            identifier_role(&result.chunk_text, &result.item_uri, &identifiers).is_definition()
        })
        .collect();
    let definition_uris: Vec<String> = results
        .iter()
        .zip(&definitions)
        .filter(|(result, defined)| {
            **defined && !is_test_chunk(&result.item_uri, &result.chunk_text)
        })
        .map(|(result, _)| result.item_uri.clone())
        .collect();
    if definition_uris.is_empty() {
        return;
    }
    for (result, defined) in results.iter_mut().zip(definitions) {
        if defined {
            continue;
        }
        let bonus = definition_uris
            .iter()
            .map(|uri| path_affinity(&result.item_uri, uri))
            .fold(0.0f32, f32::max);
        result.score += bonus;
    }
}

fn path_affinity(uri: &str, definition_uri: &str) -> f32 {
    let left = directory_of(uri);
    let right = directory_of(definition_uri);
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    if left == right {
        return SAME_DIRECTORY_AFFINITY;
    }
    let left_crate = crate_of(uri);
    let right_crate = crate_of(definition_uri);
    if !left_crate.is_empty() && left_crate == right_crate {
        return SAME_CRATE_AFFINITY;
    }
    0.0
}

fn strip_uri_path(uri: &str) -> &str {
    let rest = uri.split("://").nth(1).unwrap_or(uri);
    rest.split('@').next().unwrap_or(rest)
}

fn directory_of(uri: &str) -> &str {
    strip_uri_path(uri)
        .rsplit_once('/')
        .map(|(directory, _)| directory)
        .unwrap_or("")
}

fn crate_of(uri: &str) -> &str {
    let path = strip_uri_path(uri);
    path.find("/src/")
        .map(|index| &path[..index])
        .unwrap_or_else(|| directory_of(uri))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hybrid::candidate::Candidate;
    use crate::hybrid::config::HybridSearchConfig;
    use crate::hybrid::fusion::reciprocal_rank_fusion;
    use crate::hybrid::sample::trained_ranker;

    #[test]
    fn identifier_hits_outrank_semantic_neighbors() {
        let query = "What does should_skip_blob do?";
        let config = HybridSearchConfig::for_query(query);
        assert_eq!(config.semantic_weight, 0.45);
        let keyword = vec![Candidate::new(
            1u128,
            "fn should_skip_blob(uri: &str, sha: &str) -> bool",
            "github://acme/widgets/content/mod.rs",
            "mod.rs",
            0.07,
        )];
        let semantic = vec![Candidate::new(
            99u128,
            "generic authentication helper",
            "github://acme/widgets/auth.ts",
            "auth.ts",
            0.51,
        )];
        let fused = reciprocal_rank_fusion(keyword, semantic, &config);
        let ranked = finalize_ranking(&trained_ranker(), fused, query, 5);
        assert_eq!(ranked[0].id, 1);
    }

    #[test]
    fn finalize_ranking_caps_chunks_per_file() {
        let results = (0..4usize)
            .map(|index| {
                HybridSearchResult::new(
                    index as u128 + 1,
                    format!("fn should_skip_blob chunk {index}"),
                    "github://acme/widgets/content/mod.rs",
                    "mod.rs",
                    1.0 - index as f32 * 0.01,
                )
                .with_keyword_rank(index + 1)
                .with_keyword_score(0.1)
            })
            .collect();
        let ranked = finalize_ranking(&trained_ranker(), results, "should_skip_blob", 10);
        assert_eq!(ranked.len(), 2);
    }

    #[test]
    fn prefers_definition_chunk_over_same_file_mention() {
        let mention = HybridSearchResult::new(
            1u128,
            "identifiers: should_skip_blob, other",
            "github://acme/widgets/content/mod.rs",
            "mod.rs",
            2.0,
        )
        .with_keyword_rank(1)
        .with_keyword_score(0.4);
        let definition = HybridSearchResult::new(
            2u128,
            "pub fn should_skip_blob(&self, uri: &str, blob_sha: &str) -> bool { true }",
            "github://acme/widgets/content/mod.rs",
            "mod.rs",
            1.2,
        )
        .with_keyword_rank(2)
        .with_keyword_score(0.3);
        let ranked = finalize_ranking(
            &trained_ranker(),
            vec![mention, definition],
            "What does should_skip_blob do?",
            2,
        );
        assert!(
            ranked[0].chunk_text.contains("pub fn should_skip_blob"),
            "kept {}",
            ranked[0].chunk_text
        );
    }

    #[test]
    fn conjunction_outranks_single_identifier_definition() {
        let query = "Why must retain_content_uris use live_uris after an incremental gather?";
        let store = HybridSearchResult::new(
            1u128,
            "pub async fn retain_content_uris(&self, source_id: Uuid, uris: &[String])",
            "github://acme/widgets/embeddings/pgvector.rs",
            "pgvector.rs",
            2.0,
        )
        .with_keyword_rank(1)
        .with_keyword_score(0.5);
        let caller = HybridSearchResult::new(
            2u128,
            "store.retain_content_uris(source.id, &fetch_result.live_uris)",
            "github://acme/widgets/context/service.rs",
            "service.rs",
            1.0,
        )
        .with_keyword_rank(2)
        .with_keyword_score(0.2);
        let ranked = finalize_ranking(&trained_ranker(), vec![store, caller], query, 2);
        assert!(
            ranked[0].item_uri.contains("service.rs"),
            "kept {}",
            ranked[0].item_uri
        );
    }

    #[test]
    fn a_zero_fusion_score_takes_the_incoming_score() {
        let row = HybridSearchResult::new(1u128, "fn a() {}", "src/a.rs", "a.rs", 0.25)
            .with_fusion_score(0.0);
        let reranked = apply_local_rerank(&trained_ranker(), "a", vec![row]);
        assert_eq!(reranked[0].fusion_score, 0.25);
    }

    #[test]
    fn sort_by_score_puts_nan_last() {
        let mut results = vec![
            HybridSearchResult::new(1u128, "", "a", "a", f32::NAN),
            HybridSearchResult::new(2u128, "", "b", "b", 0.5),
            HybridSearchResult::new(3u128, "", "c", "c", 0.9),
        ];
        sort_by_score(&mut results);
        let ids: Vec<u128> = results.iter().map(|result| result.id).collect();
        assert_eq!(ids, [3, 2, 1]);
    }

    #[test]
    fn nan_scores_in_a_long_list_never_panic() {
        let scores = [
            1.0,
            12.0,
            3.0,
            11.0,
            16.0,
            f32::NAN,
            5.0,
            7.0,
            5.0,
            f32::NAN,
            15.0,
            18.0,
            f32::NAN,
            20.0,
            0.0,
            0.0,
            11.0,
            1.0,
            5.0,
            2.0,
            16.0,
        ];
        let mut results: Vec<HybridSearchResult<usize>> = scores
            .into_iter()
            .enumerate()
            .map(|(id, score)| {
                HybridSearchResult::new(id, "same text", format!("src/f{id}.rs"), "t", score)
            })
            .collect();

        prefer_definition_chunks("what is memory", &mut results);

        let mut ids: Vec<usize> = results.iter().map(|result| result.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, (0..scores.len()).collect::<Vec<_>>());
    }

    fn next_random(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn long_list_with_nan_scores(seed: u64) -> Vec<HybridSearchResult<usize>> {
        const TEXTS: [&str; 5] = [
            "pub fn load_widget(id: u32) -> Widget { fetch(id) }",
            "fn load_widget(id: u32) -> Widget { fetch(id) }",
            "let widget = load_widget(7);",
            "#[cfg(test)]\nfn loads() { assert!(load_widget(1).ok()); }",
            "widgets ship in crates of twelve",
        ];
        const ROWS: usize = 90;
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        (0..ROWS)
            .map(|id| {
                let draw = next_random(&mut state);
                let text = TEXTS[(draw % TEXTS.len() as u64) as usize];
                let similarity = ((draw >> 8) % 1000) as f32 / 1000.0;
                let semantic = if (draw >> 20).is_multiple_of(4) {
                    f32::NAN
                } else {
                    similarity
                };
                HybridSearchResult::new(id, text, format!("src/f{}.rs", id % 30), "t", similarity)
                    .with_fusion_score(similarity + 0.01)
                    .with_semantic_score(semantic)
                    .with_semantic_rank(id + 1)
            })
            .collect()
    }

    #[test]
    fn the_rerank_path_keeps_the_standard_sort_when_a_score_is_nan() {
        const QUERY: &str = "What does load_widget do?";
        let ranker = trained_ranker();
        let ids = |order: &[usize], rows: &[HybridSearchResult<usize>]| -> Vec<usize> {
            order.iter().map(|&row| rows[row].id).collect()
        };
        let mut lists_where_the_sorts_differ = 0;
        for seed in 1..=50 {
            let input = long_list_with_nan_scores(seed);
            let prepared = rescored(&ranker, QUERY, input.clone());
            let keys = definition_keys(QUERY, &prepared);
            let compare =
                |left: &usize, right: &usize| compare_definitions(&keys, &prepared, *left, *right);
            let mut standard: Vec<usize> = (0..prepared.len()).collect();
            standard.sort_by(compare);
            let mut insertion: Vec<usize> = (0..prepared.len()).collect();
            insertion_sort_by(&mut insertion, compare);
            if standard != insertion {
                lists_where_the_sorts_differ += 1;
            }

            let reranked: Vec<usize> = apply_local_rerank(&ranker, QUERY, input)
                .iter()
                .map(|result| result.id)
                .collect();

            assert_eq!(reranked, ids(&standard, &prepared), "seed {seed}");
        }
        assert!(
            lists_where_the_sorts_differ > 0,
            "no list told the two sorts apart, so this test proves nothing"
        );
    }

    #[test]
    fn nan_scores_in_a_short_list_keep_the_donor_order() {
        let scores = [
            1.0,
            12.0,
            3.0,
            11.0,
            16.0,
            f32::NAN,
            5.0,
            7.0,
            5.0,
            f32::NAN,
            15.0,
            18.0,
            f32::NAN,
            20.0,
            0.0,
            0.0,
            11.0,
            1.0,
            5.0,
            2.0,
        ];
        let mut results: Vec<HybridSearchResult<usize>> = scores
            .into_iter()
            .enumerate()
            .map(|(id, score)| {
                HybridSearchResult::new(id, "same text", format!("src/f{id}.rs"), "t", score)
            })
            .collect();

        prefer_definition_chunks("what is memory", &mut results);

        let ids: Vec<usize> = results.iter().map(|result| result.id).collect();
        assert_eq!(
            ids,
            [
                4, 1, 3, 2, 0, 5, 7, 6, 8, 9, 11, 10, 12, 13, 16, 18, 19, 17, 14, 15
            ]
        );
    }

    #[test]
    fn neighbours_of_a_definition_gain_path_affinity() {
        assert_eq!(
            path_affinity(
                "github://acme/widgets/src/inventory/bins.rs@main",
                "github://acme/widgets/src/inventory/warehouse.rs@main"
            ),
            SAME_DIRECTORY_AFFINITY
        );
        assert_eq!(
            path_affinity(
                "github://acme/widgets/src/orders/checkout.rs@main",
                "github://acme/widgets/src/inventory/warehouse.rs@main"
            ),
            SAME_CRATE_AFFINITY
        );
        assert_eq!(path_affinity("a.rs", "github://acme/widgets/b.rs"), 0.0);
    }

    #[test]
    fn reorder_applies_a_permutation() {
        let mut items = ['a', 'b', 'c', 'd'];
        reorder(&mut items, &[2, 0, 3, 1]);
        assert_eq!(items, ['c', 'a', 'd', 'b']);
    }
}
