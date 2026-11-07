use std::fmt::Debug;
use std::fs;
use std::path::PathBuf;

use abnegate_index::FEATURE_COUNT;
use abnegate_index::LinearRanker;
use abnegate_index::RankFeatures;
use abnegate_index::answers_as_definition;
use abnegate_index::blend_rank;
use abnegate_index::bridge_terms;
use abnegate_index::chunk_rank_tier;
use abnegate_index::content_tokens;
use abnegate_index::definition_misses_question;
use abnegate_index::embed_query_text;
use abnegate_index::extract_features;
use abnegate_index::first_stage_answered;
use abnegate_index::fit_pairwise;
use abnegate_index::identifier_hit_count;
use abnegate_index::identifier_part_coverage;
use abnegate_index::identifier_role;
use abnegate_index::is_fixture_uri;
use abnegate_index::is_module_prelude;
use abnegate_index::is_outline_chunk;
use abnegate_index::is_test_chunk;
use abnegate_index::lexical_cross_score;
use abnegate_index::path_uri_patterns;
use abnegate_index::path_uri_tokens;
use abnegate_index::question_phrases;
use abnegate_index::rewrite_query;
use abnegate_index::role_strength;
use abnegate_index::sanitize_search_query;
use abnegate_index::score_hit;
use serde_json::Value;

const MAXIMUM_ULPS: i64 = 4;
const ABSOLUTE_TOLERANCE: f32 = 1e-6;
const TRAINED_RELATIVE_TOLERANCE: f32 = 1e-5;
const TRAINED_ABSOLUTE_TOLERANCE: f32 = 1e-7;
const STRICT: bool = cfg!(all(target_os = "macos", target_arch = "aarch64"));

fn trained_ranker() -> LinearRanker {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ranking/trained_ranker.rs"
    ))
}

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ranking")
        .join(name);
    let contents = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&contents)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn text(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("expected a string, got {value}"))
}

fn list(value: &Value) -> &[Value] {
    value
        .as_array()
        .unwrap_or_else(|| panic!("expected an array, got {value}"))
}

fn strings(value: &Value) -> Vec<String> {
    list(value)
        .iter()
        .map(|item| text(item).to_string())
        .collect()
}

fn float(value: &Value) -> f32 {
    match value {
        Value::String(text) if text == "NaN" => f32::NAN,
        other => other
            .as_f64()
            .unwrap_or_else(|| panic!("expected a number, got {other}")) as f32,
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

fn golden_float(value: &Value) -> f32 {
    let hexadecimal = text(value)
        .strip_prefix("0x")
        .unwrap_or_else(|| panic!("expected 0x-prefixed bits, got {value}"));
    f32::from_bits(
        u32::from_str_radix(hexadecimal, 16)
            .unwrap_or_else(|error| panic!("bits {value}: {error}")),
    )
}

fn ordered(value: f32) -> i64 {
    let bits = value.to_bits();
    if bits & 0x8000_0000 == 0 {
        i64::from(bits)
    } else {
        -i64::from(bits & 0x7fff_ffff)
    }
}

fn within_ulps_or_absolute(actual: f32, expected: f32) -> bool {
    if actual.to_bits() == expected.to_bits() {
        return true;
    }
    if actual.is_nan() || expected.is_nan() {
        return false;
    }
    (ordered(actual) - ordered(expected)).abs() <= MAXIMUM_ULPS
        || (actual - expected).abs() <= ABSOLUTE_TOLERANCE
}

fn within_trained_tolerance(actual: f32, expected: f32) -> bool {
    actual.to_bits() == expected.to_bits()
        || (actual - expected).abs()
            <= TRAINED_RELATIVE_TOLERANCE * expected.abs() + TRAINED_ABSOLUTE_TOLERANCE
}

struct Row {
    context: String,
    tolerant: Vec<String>,
    strict: Vec<String>,
}

impl Row {
    fn exact<T: PartialEq + Debug>(&mut self, field: &str, actual: T, expected: T) {
        if actual != expected {
            let mismatch = format!(
                "{} {field}: got {actual:?}, want {expected:?}",
                self.context
            );
            self.tolerant.push(mismatch.clone());
            self.strict.push(mismatch);
        }
    }

    fn exact_bits(&mut self, field: &str, actual: f32, expected: &Value) {
        let expected = golden_float(expected);
        self.exact(field, BitsOf(actual), BitsOf(expected));
    }

    fn close(&mut self, field: &str, actual: f32, expected: &Value, within: fn(f32, f32) -> bool) {
        let expected = golden_float(expected);
        if actual.to_bits() == expected.to_bits() {
            return;
        }
        let mismatch = format!(
            "{} {field}: got {actual:?} ({:#010x}), want {expected:?} ({:#010x})",
            self.context,
            actual.to_bits(),
            expected.to_bits()
        );
        if !within(actual, expected) {
            self.tolerant.push(mismatch.clone());
        }
        self.strict.push(mismatch);
    }
}

struct BitsOf(f32);

impl Debug for BitsOf {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?} ({:#010x})", self.0, self.0.to_bits())
    }
}

impl PartialEq<BitsOf> for BitsOf {
    fn eq(&self, other: &BitsOf) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

struct Replay {
    section: &'static str,
    rows: usize,
    tolerant_passing: usize,
    strict_passing: usize,
    tolerant: Vec<String>,
    strict: Vec<String>,
}

impl Replay {
    fn new(section: &'static str) -> Self {
        Self {
            section,
            rows: 0,
            tolerant_passing: 0,
            strict_passing: 0,
            tolerant: Vec::new(),
            strict: Vec::new(),
        }
    }

    fn row(&mut self, context: String, check: impl FnOnce(&mut Row)) {
        let mut row = Row {
            context,
            tolerant: Vec::new(),
            strict: Vec::new(),
        };
        check(&mut row);
        self.rows += 1;
        if row.tolerant.is_empty() {
            self.tolerant_passing += 1;
        }
        if row.strict.is_empty() {
            self.strict_passing += 1;
        }
        self.tolerant.extend(row.tolerant);
        self.strict.extend(row.strict);
    }

    fn finish(self, expected_rows: usize) {
        eprintln!(
            "{}: {} rows replayed, {} pass tolerant, {} pass bit-exact",
            self.section, self.rows, self.tolerant_passing, self.strict_passing
        );
        assert_eq!(
            self.rows, expected_rows,
            "{} replayed {} rows but the golden holds {expected_rows}",
            self.section, self.rows
        );
        assert!(
            self.tolerant.is_empty(),
            "{} differs from the donor beyond tolerance:\n{}",
            self.section,
            self.tolerant.join("\n")
        );
        if STRICT {
            assert!(
                self.strict.is_empty(),
                "{} is not bit-exact with the donor on its capture platform:\n{}",
                self.section,
                self.strict.join("\n")
            );
        }
    }
}

struct Candidate<'a> {
    label: &'a str,
    uri: &'a str,
    title: &'a str,
    text: &'a str,
    semantic: Option<f32>,
    keyword: Option<f32>,
    keyword_rank: Option<usize>,
    semantic_rank: Option<usize>,
    fusion: f32,
}

impl<'a> Candidate<'a> {
    fn parse(value: &'a Value) -> Self {
        Self {
            label: text(&value["label"]),
            uri: text(&value["uri"]),
            title: text(&value["title"]),
            text: text(&value["text"]),
            semantic: optional_float(&value["semantic"]),
            keyword: optional_float(&value["keyword"]),
            keyword_rank: optional_usize(&value["keyword_rank"]),
            semantic_rank: optional_usize(&value["semantic_rank"]),
            fusion: float(&value["fusion"]),
        }
    }

    fn features(&self, query: &str) -> RankFeatures {
        extract_features(
            query,
            self.uri,
            self.title,
            self.text,
            self.semantic,
            self.keyword,
            self.keyword_rank,
            self.semantic_rank,
            self.fusion,
        )
    }

    fn score(&self, ranker: &LinearRanker, query: &str) -> f32 {
        score_hit(
            ranker,
            query,
            self.uri,
            self.title,
            self.text,
            self.semantic,
            self.keyword,
            self.keyword_rank,
            self.semantic_rank,
            self.fusion,
        )
    }
}

struct Cases {
    root: Value,
    golden: Value,
}

impl Cases {
    fn load() -> Self {
        Self {
            root: fixture("cases.json"),
            golden: fixture("golden.json"),
        }
    }

    fn candidates(&self) -> Vec<Candidate<'_>> {
        list(&self.root["candidates"])
            .iter()
            .map(Candidate::parse)
            .collect()
    }

    fn query(&self, label: &str) -> &str {
        list(&self.root["queries"])
            .iter()
            .find(|query| text(&query["label"]) == label)
            .map(|query| text(&query["text"]))
            .unwrap_or_else(|| panic!("unknown query {label}"))
    }
}

fn find<'a, 'b>(candidates: &'b [Candidate<'a>], label: &str) -> &'b Candidate<'a> {
    candidates
        .iter()
        .find(|candidate| candidate.label == label)
        .unwrap_or_else(|| panic!("unknown candidate {label}"))
}

#[test]
fn query_rewriting_matches_the_donor_for_every_query() {
    let cases = Cases::load();
    let queries = list(&cases.root["queries"]);
    let goldens = list(&cases.golden["queries"]);
    let mut replay = Replay::new("queries");
    for (query, golden) in queries.iter().zip(goldens) {
        let label = text(&query["label"]);
        let query = text(&query["text"]);
        replay.row(format!("query {label}"), |row| {
            row.exact("label", label, text(&golden["label"]));
            let rewritten = rewrite_query(query);
            row.exact(
                "rewrite.original",
                rewritten.original.as_str(),
                text(&golden["rewrite"]["original"]),
            );
            row.exact(
                "rewrite.keyword",
                rewritten.keyword.as_str(),
                text(&golden["rewrite"]["keyword"]),
            );
            row.exact(
                "rewrite.identifiers",
                rewritten.identifiers,
                strings(&golden["rewrite"]["identifiers"]),
            );
            row.exact(
                "sanitized",
                sanitize_search_query(query).as_str(),
                text(&golden["sanitized"]),
            );
            row.exact(
                "bridge_terms",
                bridge_terms(query),
                strings(&golden["bridge_terms"]),
            );
            row.exact(
                "content_tokens",
                content_tokens(query),
                strings(&golden["content_tokens"]),
            );
            row.exact(
                "question_phrases",
                question_phrases(query),
                strings(&golden["question_phrases"]),
            );
            row.exact(
                "path_uri_tokens",
                path_uri_tokens(query),
                strings(&golden["path_uri_tokens"]),
            );
            row.exact(
                "path_uri_patterns",
                path_uri_patterns(query),
                strings(&golden["path_uri_patterns"]),
            );
            let embed = golden["embed"]
                .as_object()
                .unwrap_or_else(|| panic!("query {label} has no embed map"));
            assert!(!embed.is_empty(), "query {label} embeds no models");
            for (model, expected) in embed {
                row.exact(
                    &format!("embed {model}"),
                    embed_query_text(model, query).as_str(),
                    text(expected),
                );
            }
        });
    }
    replay.finish(queries.len().max(goldens.len()));
}

#[test]
fn scored_pairs_match_the_donor_with_the_trained_ranker() {
    let cases = Cases::load();
    let candidates = cases.candidates();
    let ranker = trained_ranker();
    let goldens = list(&cases.golden["scored"]);
    let mut replay = Replay::new("scored");
    let mut index = 0;
    for group in list(&cases.root["scored"]) {
        let query_label = text(&group["query"]);
        let query = cases.query(query_label);
        let identifiers = rewrite_query(query).identifiers;
        for label in list(&group["candidates"]) {
            let candidate = find(&candidates, text(label));
            let golden = goldens
                .get(index)
                .unwrap_or_else(|| panic!("no golden for scored row {index}"));
            index += 1;
            replay.row(format!("scored {query_label}/{}", candidate.label), |row| {
                row.exact("query", query_label, text(&golden["query"]));
                row.exact("candidate", candidate.label, text(&golden["candidate"]));
                let features = candidate.features(query);
                let expected_features = list(&golden["features"]);
                row.exact("feature count", expected_features.len(), FEATURE_COUNT);
                for (position, (actual, expected)) in
                    features.values.iter().zip(expected_features).enumerate()
                {
                    row.close(
                        &format!("features[{position}]"),
                        *actual,
                        expected,
                        within_ulps_or_absolute,
                    );
                }
                row.close(
                    "score_hit",
                    candidate.score(&ranker, query),
                    &golden["score_hit"],
                    within_ulps_or_absolute,
                );
                row.exact_bits(
                    "lexical_cross_score",
                    lexical_cross_score(query, candidate.uri, candidate.title, candidate.text),
                    &golden["lexical_cross_score"],
                );
                row.exact(
                    "identifier_role",
                    format!(
                        "{:?}",
                        identifier_role(candidate.text, candidate.uri, &identifiers)
                    )
                    .as_str(),
                    text(&golden["identifier_role"]),
                );
                row.exact(
                    "chunk_rank_tier",
                    Some(u64::from(chunk_rank_tier(
                        query,
                        candidate.uri,
                        candidate.text,
                    ))),
                    golden["chunk_rank_tier"].as_u64(),
                );
                row.exact(
                    "role_strength",
                    Some(u64::from(role_strength(
                        candidate.text,
                        candidate.uri,
                        &identifiers,
                    ))),
                    golden["role_strength"].as_u64(),
                );
                row.exact(
                    "answers_as_definition",
                    Some(answers_as_definition(query, candidate.uri, candidate.text)),
                    golden["answers_as_definition"].as_bool(),
                );
                row.exact(
                    "definition_misses_question",
                    Some(definition_misses_question(query, candidate.text)),
                    golden["definition_misses_question"].as_bool(),
                );
                row.exact(
                    "is_outline_chunk",
                    Some(is_outline_chunk(candidate.text)),
                    golden["is_outline_chunk"].as_bool(),
                );
                row.exact(
                    "is_module_prelude",
                    Some(is_module_prelude(candidate.text)),
                    golden["is_module_prelude"].as_bool(),
                );
                row.exact(
                    "is_test_chunk",
                    Some(is_test_chunk(candidate.uri, candidate.text)),
                    golden["is_test_chunk"].as_bool(),
                );
                row.exact(
                    "is_fixture_uri",
                    Some(is_fixture_uri(candidate.uri)),
                    golden["is_fixture_uri"].as_bool(),
                );
                row.exact(
                    "identifier_hit_count",
                    Some(identifier_hit_count(candidate.text, &identifiers) as u64),
                    golden["identifier_hit_count"].as_u64(),
                );
                row.exact_bits(
                    "identifier_part_coverage",
                    identifier_part_coverage(candidate.text, &identifiers),
                    &golden["identifier_part_coverage"],
                );
            });
        }
    }
    replay.finish(goldens.len());
}

#[test]
fn first_stage_answers_match_the_donor() {
    let cases = Cases::load();
    let candidates = cases.candidates();
    let inputs = list(&cases.root["first_stage"]);
    let goldens = list(&cases.golden["first_stage"]);
    let mut replay = Replay::new("first_stage");
    for (position, (input, golden)) in inputs.iter().zip(goldens).enumerate() {
        let query = text(&input["query"]);
        replay.row(format!("first_stage[{position}]"), |row| {
            row.exact("query", query, text(&golden["query"]));
            row.exact("hits", &input["hits"], &golden["hits"]);
            let hits = list(&input["hits"])
                .iter()
                .map(|label| find(&candidates, text(label)))
                .map(|candidate| (candidate.uri, candidate.text));
            row.exact(
                "answered",
                Some(first_stage_answered(query, hits)),
                golden["answered"].as_bool(),
            );
        });
    }
    replay.finish(inputs.len().max(goldens.len()));
}

#[test]
fn pairwise_training_matches_the_donor_weights() {
    let cases = Cases::load();
    let candidates = cases.candidates();
    let training = &cases.root["training"];
    let pairs: Vec<(RankFeatures, RankFeatures)> = list(&training["pairs"])
        .iter()
        .map(|pair| {
            let query = text(&pair["query"]);
            (
                find(&candidates, text(&pair["positive"])).features(query),
                find(&candidates, text(&pair["negative"])).features(query),
            )
        })
        .collect();
    assert!(!pairs.is_empty(), "the training section has no pairs");
    let steps = training["steps"].as_u64().expect("training steps") as usize;
    let fitted = fit_pairwise(&pairs, steps, float(&training["learning_rate"]));
    let golden = &cases.golden["training"];
    let mut replay = Replay::new("training");
    replay.row("fit_pairwise".to_string(), |row| {
        let expected = list(&golden["weights"]);
        row.exact("weight count", expected.len(), FEATURE_COUNT);
        for (position, (actual, expected)) in fitted.weights.iter().zip(expected).enumerate() {
            row.close(
                &format!("weights[{position}]"),
                *actual,
                expected,
                within_trained_tolerance,
            );
        }
        row.close(
            "bias",
            fitted.bias,
            &golden["bias"],
            within_trained_tolerance,
        );
    });
    replay.finish(1);
}

#[test]
fn blended_ranks_match_the_donor_bit_for_bit() {
    let cases = Cases::load();
    let inputs = list(&cases.root["blend"]);
    let goldens = list(&cases.golden["blend"]);
    let mut replay = Replay::new("blend");
    for (position, (input, golden)) in inputs.iter().zip(goldens).enumerate() {
        replay.row(format!("blend[{position}]"), |row| {
            row.exact("learned", &input["learned"], &golden["learned"]);
            row.exact(
                "cross_encoder",
                &input["cross_encoder"],
                &golden["cross_encoder"],
            );
            row.exact_bits(
                "result",
                blend_rank(float(&input["learned"]), float(&input["cross_encoder"])),
                &golden["result"],
            );
        });
    }
    replay.finish(inputs.len().max(goldens.len()));
}

#[test]
fn trained_ranker_fixture_holds_the_donor_default_ranker() {
    let cases = Cases::load();
    let ranker = trained_ranker();
    let golden = &cases.golden["default_ranker"];
    let mut replay = Replay::new("default_ranker");
    replay.row("default_ranker".to_string(), |row| {
        let expected = list(&golden["weights"]);
        row.exact("weight count", expected.len(), FEATURE_COUNT);
        for (position, (actual, expected)) in ranker.weights.iter().zip(expected).enumerate() {
            row.exact_bits(&format!("weights[{position}]"), *actual, expected);
        }
        row.exact_bits("bias", ranker.bias, &golden["bias"]);
    });
    replay.finish(1);
}
