use std::fs;
use std::path::PathBuf;

use abnegate_index::rewrite_query;
use serde_json::Value;

fn retrieval_cases() -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/retrieval.json");
    let contents = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let set: Value = serde_json::from_str(&contents)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
    set["cases"]
        .as_array()
        .unwrap_or_else(|| panic!("{} has no cases array", path.display()))
        .clone()
}

fn field<'a>(case: &'a Value, name: &str) -> &'a str {
    case[name]
        .as_str()
        .unwrap_or_else(|| panic!("case {case} has no string {name}"))
}

#[test]
fn every_expected_identifier_survives_query_rewriting() {
    let cases = retrieval_cases();
    assert!(!cases.is_empty(), "retrieval.json holds no cases");
    let mut checked = 0;
    for case in &cases {
        let id = field(case, "id");
        let rewritten = rewrite_query(field(case, "query"));
        let expected = case["expect_identifiers"]
            .as_array()
            .unwrap_or_else(|| panic!("case {id} has no expect_identifiers"));
        for expected in expected {
            let expected = expected
                .as_str()
                .unwrap_or_else(|| panic!("case {id} expects a non-string identifier"));
            assert!(
                rewritten
                    .identifiers
                    .iter()
                    .any(|identifier| identifier == expected),
                "case {id} expected identifier {expected:?} in {:?}",
                rewritten.identifiers
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "retrieval.json expects no identifiers");
}
