use std::fs;

use abnegate_index::Candidate;
use abnegate_index::HybridSearchConfig;
use abnegate_index::HybridSearchResult;
use abnegate_index::LinearRanker;
use abnegate_index::apply_local_rerank;
use abnegate_index::cap_per_file;
use abnegate_index::finalize_ranking;
use abnegate_index::identifier_file_patterns;
use abnegate_index::keep_answer_chunks;
use abnegate_index::prefer_definition_chunks;
use abnegate_index::reciprocal_rank_fusion;
use abnegate_index::rewrite_query;
use abnegate_index::same_file_patterns;
use abnegate_index::sort_by_score;
use abnegate_index::uri_stem_matches_identifier;
use serde_json::Value;

const MAXIMUM_ULPS: u32 = 4;

const ABSOLUTE_TOLERANCE: f32 = 1e-6;

const STAGES: [&str; 5] = [
    "finalize_ranking",
    "apply_local_rerank",
    "prefer_definition_chunks",
    "cap_per_file",
    "sort_by_score",
];

const RERANKED_STAGES: [&str; 2] = ["finalize_ranking", "apply_local_rerank"];

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/ranking/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let content = fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    serde_json::from_str(&content).unwrap_or_else(|error| panic!("parse {path}: {error}"))
}

fn trained_ranker() -> LinearRanker {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ranking/trained_ranker.rs"
    ))
}

fn text(value: &Value) -> &str {
    value.as_str().expect("string field")
}

fn float(value: &Value) -> f32 {
    match value {
        Value::String(text) if text == "NaN" => f32::NAN,
        other => other.as_f64().expect("float field") as f32,
    }
}

fn optional_float(value: &Value) -> Option<f32> {
    if value.is_null() {
        None
    } else {
        Some(float(value))
    }
}

fn optional_usize(value: &Value) -> Option<usize> {
    value.as_u64().map(|value| value as usize)
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("string list")
        .iter()
        .map(|item| text(item).to_string())
        .collect()
}

fn golden_bits(value: &Value) -> u32 {
    let hex = text(value)
        .strip_prefix("0x")
        .expect("0x-prefixed f32 bits");
    u32::from_str_radix(hex, 16).expect("hex f32 bits")
}

fn optional_golden_bits(value: &Value) -> Option<u32> {
    if value.is_null() {
        None
    } else {
        Some(golden_bits(value))
    }
}

fn ordered(value: f32) -> i64 {
    let bits = value.to_bits() as i32;
    if bits < 0 {
        i64::from(i32::MIN) - i64::from(bits)
    } else {
        i64::from(bits)
    }
}

fn close(actual: f32, expected: f32) -> bool {
    if actual.is_nan() || expected.is_nan() {
        return actual.is_nan() && expected.is_nan();
    }
    (ordered(actual) - ordered(expected)).unsigned_abs() <= u64::from(MAXIMUM_ULPS)
        || (actual - expected).abs() <= ABSOLUTE_TOLERANCE
}

fn assert_close(context: &str, actual: f32, expected_bits: u32) {
    let expected = f32::from_bits(expected_bits);
    assert!(
        close(actual, expected),
        "{context}: {actual} (0x{:08x}) is not within {MAXIMUM_ULPS} ULP or {ABSOLUTE_TOLERANCE} of {expected} (0x{expected_bits:08x})",
        actual.to_bits()
    );
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    assert_eq!(
        actual.to_bits(),
        expected_bits,
        "{context}: bits differ on the capture platform"
    );
}

fn assert_bits(context: &str, actual: f32, expected_bits: u32) {
    assert_eq!(
        actual.to_bits(),
        expected_bits,
        "{context}: {actual} differs from {}",
        f32::from_bits(expected_bits)
    );
}

fn list_results(list: &Value, labels: &[&str]) -> Vec<HybridSearchResult<usize>> {
    list["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|result| {
            let label = text(&result["label"]);
            let id = labels
                .iter()
                .position(|known| *known == label)
                .unwrap_or_else(|| panic!("unknown result label {label}"));
            let mut row = HybridSearchResult::new(
                id,
                text(&result["text"]),
                text(&result["uri"]),
                text(&result["title"]),
                float(&result["score"]),
            )
            .with_fusion_score(float(&result["fusion_score"]));
            row.keyword_rank = optional_usize(&result["keyword_rank"]);
            row.semantic_rank = optional_usize(&result["semantic_rank"]);
            row.keyword_score = optional_float(&result["keyword_score"]);
            row.semantic_score = optional_float(&result["semantic_score"]);
            row
        })
        .collect()
}

fn run_stage(
    stage: &str,
    ranker: &LinearRanker,
    input: &[HybridSearchResult<usize>],
    query: &str,
    limit: usize,
    per_file: usize,
) -> Vec<HybridSearchResult<usize>> {
    let input = input.to_vec();
    match stage {
        "finalize_ranking" => finalize_ranking(ranker, input, query, limit),
        "apply_local_rerank" => apply_local_rerank(ranker, query, input),
        "prefer_definition_chunks" => {
            let mut preferred = input;
            prefer_definition_chunks(query, &mut preferred);
            preferred
        }
        "cap_per_file" => cap_per_file(input, per_file, limit, query),
        "sort_by_score" => {
            let mut sorted = input;
            sort_by_score(&mut sorted);
            sorted
        }
        other => panic!("unknown stage {other}"),
    }
}

#[test]
fn every_list_stage_reproduces_the_donor_order_and_scores() {
    let cases = fixture("cases.json");
    let golden = fixture("golden.json");
    let ranker = trained_ranker();
    let labels: Vec<&str> = cases["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|candidate| text(&candidate["label"]))
        .collect();
    let lists = cases["lists"].as_array().expect("lists");
    let golden_lists = golden["lists"].as_array().expect("golden lists");
    assert_eq!(lists.len(), golden_lists.len());
    for list in lists {
        let name = text(&list["name"]);
        let query = text(&list["query"]);
        let limit = list["limit"].as_u64().expect("limit") as usize;
        let per_file = list["per_file"].as_u64().expect("per_file") as usize;
        let expected_list = golden_lists
            .iter()
            .find(|golden| golden["name"] == list["name"])
            .unwrap_or_else(|| panic!("no golden for list {name}"));
        let input = list_results(list, &labels);
        for stage in STAGES {
            let actual = run_stage(stage, &ranker, &input, query, limit, per_file);
            let expected = expected_list["stages"][stage]
                .as_array()
                .unwrap_or_else(|| panic!("no {stage} golden for list {name}"));
            let actual_labels: Vec<&str> = actual.iter().map(|row| labels[row.id]).collect();
            let expected_labels: Vec<&str> =
                expected.iter().map(|row| text(&row["label"])).collect();
            assert_eq!(actual_labels, expected_labels, "{name} {stage} order");
            for (row, expected_row) in actual.iter().zip(expected) {
                let context = format!("{name} {stage} {}", labels[row.id]);
                assert_bits(
                    &format!("{context} fusion_score"),
                    row.fusion_score,
                    golden_bits(&expected_row["fusion_score"]),
                );
                let expected_score = golden_bits(&expected_row["score"]);
                if RERANKED_STAGES.contains(&stage) {
                    assert_close(&format!("{context} score"), row.score, expected_score);
                } else {
                    assert_bits(&format!("{context} score"), row.score, expected_score);
                }
            }
        }
    }
}

fn leg(case: &Value, name: &str, labels: &mut Vec<String>) -> Vec<Candidate<usize>> {
    case[name]
        .as_array()
        .unwrap_or_else(|| panic!("{name} leg"))
        .iter()
        .map(|item| {
            let label = text(&item["label"]);
            let id = labels
                .iter()
                .position(|known| known == label)
                .unwrap_or_else(|| {
                    labels.push(label.to_string());
                    labels.len() - 1
                });
            Candidate::new(
                id,
                text(&item["text"]),
                text(&item["uri"]),
                text(&item["title"]),
                float(&item["score"]),
            )
        })
        .collect()
}

#[test]
fn fusion_reproduces_the_donor_ranks_and_scores() {
    let cases = fixture("cases.json");
    let golden = fixture("golden_private.json");
    let fusion = cases["fusion"].as_array().expect("fusion");
    let expected_fusion = golden["fusion"].as_array().expect("golden fusion");
    assert_eq!(fusion.len(), expected_fusion.len());
    for (index, (case, expected)) in fusion.iter().zip(expected_fusion).enumerate() {
        let mut labels = Vec::new();
        let keyword = leg(case, "keyword", &mut labels);
        let semantic = leg(case, "semantic", &mut labels);
        let config = HybridSearchConfig::default()
            .with_semantic_weight(float(&case["semantic_weight"]))
            .with_fusion_constant(float(&case["fusion_constant"]));
        let results = reciprocal_rank_fusion(keyword, semantic, &config);
        let expected = expected["results"].as_array().expect("fusion results");
        let actual_labels: Vec<&str> = results.iter().map(|row| labels[row.id].as_str()).collect();
        let expected_labels: Vec<&str> = expected.iter().map(|row| text(&row["label"])).collect();
        assert_eq!(actual_labels, expected_labels, "fusion case {index} order");
        for (row, expected_row) in results.iter().zip(expected) {
            let context = format!("fusion case {index} {}", labels[row.id]);
            assert_eq!(
                row.keyword_rank,
                optional_usize(&expected_row["keyword_rank"]),
                "{context} keyword_rank"
            );
            assert_eq!(
                row.semantic_rank,
                optional_usize(&expected_row["semantic_rank"]),
                "{context} semantic_rank"
            );
            assert_eq!(
                row.keyword_score.map(f32::to_bits),
                optional_golden_bits(&expected_row["keyword_score"]),
                "{context} keyword_score"
            );
            assert_eq!(
                row.semantic_score.map(f32::to_bits),
                optional_golden_bits(&expected_row["semantic_score"]),
                "{context} semantic_score"
            );
            assert_bits(
                &format!("{context} score"),
                row.score,
                golden_bits(&expected_row["score"]),
            );
            assert_bits(
                &format!("{context} fusion_score"),
                row.fusion_score,
                golden_bits(&expected_row["fusion_score"]),
            );
        }
    }
}

struct AnswerItem {
    label: String,
    uri: String,
    text: String,
}

#[test]
fn keep_answer_chunks_keeps_the_donor_labels() {
    let cases = fixture("cases.json");
    let golden = fixture("golden_private.json");
    let mut expected = golden["answers"].as_array().expect("golden answers").iter();
    for (case_index, case) in cases["answers"]
        .as_array()
        .expect("answers")
        .iter()
        .enumerate()
    {
        for limit in case["limits"].as_array().expect("limits") {
            let limit = limit.as_u64().expect("limit") as usize;
            for query in case["queries"].as_array().expect("queries") {
                let query = text(query);
                let items: Vec<AnswerItem> = case["items"]
                    .as_array()
                    .expect("items")
                    .iter()
                    .map(|item| AnswerItem {
                        label: text(&item["label"]).to_string(),
                        uri: text(&item["uri"]).to_string(),
                        text: text(&item["text"]).to_string(),
                    })
                    .collect();
                let identifiers = rewrite_query(query).identifiers;
                let kept = keep_answer_chunks(items, limit, &identifiers, |item| {
                    (item.text.as_str(), item.uri.as_str())
                });
                let golden = expected.next().expect("a golden for every answer case");
                assert_eq!(golden["case"].as_u64(), Some(case_index as u64));
                assert_eq!(golden["limit"].as_u64(), Some(limit as u64));
                assert_eq!(text(&golden["query"]), query);
                let labels: Vec<String> = kept.into_iter().map(|item| item.label).collect();
                assert_eq!(
                    labels,
                    strings(&golden["kept"]),
                    "answers case {case_index} limit {limit} {query:?}"
                );
            }
        }
    }
    assert!(expected.next().is_none(), "unreplayed answer goldens");
}

#[test]
fn same_file_patterns_match_the_donor() {
    let cases = fixture("cases.json");
    let golden = fixture("golden_private.json");
    let queries = cases["queries"].as_array().expect("queries");
    let patterns = golden["patterns"].as_array().expect("golden patterns");
    assert_eq!(queries.len(), patterns.len());
    for (query, expected) in queries.iter().zip(patterns) {
        assert_eq!(query["label"], expected["query"]);
        let query_text = text(&query["text"]);
        assert_eq!(
            same_file_patterns(query_text),
            strings(&expected["same_file_patterns"]),
            "same_file_patterns {query_text:?}"
        );
        assert_eq!(
            identifier_file_patterns(query_text),
            strings(&expected["identifier_file_patterns"]),
            "identifier_file_patterns {query_text:?}"
        );
    }
}

#[test]
fn uri_stems_match_the_donor() {
    let cases = fixture("cases.json");
    let golden = fixture("golden_private.json");
    let stems = cases["stems"].as_array().expect("stems");
    let expected_stems = golden["stems"].as_array().expect("golden stems");
    assert_eq!(stems.len(), expected_stems.len());
    for (case, expected) in stems.iter().zip(expected_stems) {
        assert_eq!(case["uri"], expected["uri"]);
        let identifiers = strings(&case["identifiers"]);
        assert_eq!(identifiers, strings(&expected["identifiers"]));
        assert_eq!(
            uri_stem_matches_identifier(text(&case["uri"]), &identifiers),
            expected["matches"].as_bool().expect("matches"),
            "{} {identifiers:?}",
            text(&case["uri"])
        );
    }
}

#[test]
fn for_query_reproduces_the_donor_config() {
    let cases = fixture("cases.json");
    let golden = fixture("golden.json");
    let queries = cases["queries"].as_array().expect("queries");
    let expected_queries = golden["queries"].as_array().expect("golden queries");
    assert_eq!(queries.len(), expected_queries.len());
    for (query, expected) in queries.iter().zip(expected_queries) {
        assert_eq!(query["label"], expected["label"]);
        let query_text = text(&query["text"]);
        let config = HybridSearchConfig::for_query(query_text);
        let expected = &expected["config"];
        let context = format!("for_query {query_text:?}");
        assert_bits(
            &format!("{context} semantic_weight"),
            config.semantic_weight,
            golden_bits(&expected["semantic_weight"]),
        );
        assert_bits(
            &format!("{context} fusion_constant"),
            config.fusion_constant,
            golden_bits(&expected["fusion_constant"]),
        );
        assert_bits(
            &format!("{context} minimum_keyword_score"),
            config.minimum_keyword_score,
            golden_bits(&expected["minimum_keyword_score"]),
        );
        assert_bits(
            &format!("{context} minimum_semantic_score"),
            config.minimum_semantic_score,
            golden_bits(&expected["minimum_semantic_score"]),
        );
    }
}

#[test]
fn ulp_tolerance_accepts_neighbours_and_rejects_distant_values() {
    let base = 16.0f32;
    assert!(close(base, f32::from_bits(base.to_bits() + MAXIMUM_ULPS)));
    assert!(!close(
        base,
        f32::from_bits(base.to_bits() + MAXIMUM_ULPS + 1)
    ));
    assert!(close(1e-9, -1e-9));
    assert!(close(f32::NAN, f32::NAN));
    assert!(!close(f32::NAN, 0.0));
}
