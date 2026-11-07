//! What kind of chunk a hit is: a test, a fixture, an outline, a prelude, a migration.

use crate::query::rewrite_query;
use crate::rank::definition::code_definition;
use crate::rank::identifier::identifier_hit_count;

const PRELUDE_IMPLEMENTATION_MARKERS: &[&str] = &[
    "\npub fn ",
    "\npub async fn ",
    "\nfn ",
    "\nimpl ",
    "\npub struct ",
    "\npub enum ",
];

/// How well a chunk answers `query`, higher is better, used to pack real
/// answers ahead of tests and outlines.
///
/// 0 is a test or fixture; 1 an outline that defines nothing; 5 a definition
/// containing every one of two or more code identifiers; 4 every such
/// identifier without a definition; 3 a definition; 2 anything else.
pub fn chunk_rank_tier(query: &str, uri: &str, text: &str) -> u8 {
    let identifiers = rewrite_query(query).identifiers;
    if is_test_chunk(uri, text) || is_fixture_uri(uri) {
        return 0;
    }
    let defined = code_definition(text, uri, &identifiers);
    if is_outline_chunk(text) && !defined {
        return 1;
    }
    let code_identifiers: Vec<String> = identifiers
        .iter()
        .filter(|identifier| !identifier.contains('/'))
        .cloned()
        .collect();
    let covered_code = identifier_hit_count(text, &code_identifiers);
    let all_code = code_identifiers.len() >= 2 && covered_code == code_identifiers.len();
    if defined && all_code {
        return 5;
    }
    if all_code {
        return 4;
    }
    if defined {
        return 3;
    }
    2
}

/// Whether the first 400 characters mark a file-header or API-surface
/// outline chunk.
pub fn is_outline_chunk(text: &str) -> bool {
    let head: String = text.chars().take(400).collect();
    head.contains("kind: file_header") || head.contains("kind: api_surface")
}

/// Whether the chunk is only a file banner or module documentation: a
/// top-level chunk with no function, `impl`, public struct or enum, and no SQL
/// definition.
pub fn is_module_prelude(text: &str) -> bool {
    let head: String = text.chars().take(400).collect();
    if !head.contains("kind: top_level") {
        return false;
    }
    if text.contains("CREATE TABLE") || text.contains("CREATE OR REPLACE") {
        return false;
    }
    let has_implementation = PRELUDE_IMPLEMENTATION_MARKERS
        .iter()
        .any(|needle| text.contains(needle));
    !has_implementation
}

/// Whether `uri` is an evaluation fixture, a test, or a cached SQL query file.
pub fn is_fixture_uri(uri: &str) -> bool {
    fixture_penalty(&uri.to_ascii_lowercase()) > 0.0
}

/// Whether the chunk is test code, judged by its URI, its symbol header in
/// the first 500 characters, or test attributes and assertions in its text.
pub fn is_test_chunk(uri: &str, text: &str) -> bool {
    let lowercase_uri = uri.to_ascii_lowercase();
    if lowercase_uri.contains("/tests/")
        || lowercase_uri.contains(".test.")
        || lowercase_uri.contains("_test.rs")
        || lowercase_uri.contains("/e2e/")
        || lowercase_uri.contains(".sqlx/")
    {
        return true;
    }
    let head: String = text.chars().take(500).collect();
    let lowercase_head = head.to_ascii_lowercase();
    lowercase_head.contains("symbol: tests\n")
        || lowercase_head.contains("symbol: tests.")
        || lowercase_head.contains("parent: tests")
        || lowercase_head.contains("parents: tests")
        || text.contains("#[cfg(test)]")
        || head.contains("#[test]")
        || text.contains("assert_eq!(")
        || text.contains("assert_ne!(")
        || text.contains("assert!(")
}

pub(super) fn fixture_penalty(lowercase_uri: &str) -> f32 {
    if lowercase_uri.contains("/evals/")
        || lowercase_uri.contains("retrieval.json")
        || lowercase_uri.contains("/tests/")
        || lowercase_uri.contains(".test.")
        || lowercase_uri.contains(".sqlx/")
    {
        1.0
    } else {
        0.0
    }
}

pub(super) fn looks_like_unit_test_body(text: &str) -> bool {
    text.contains("assert_eq!(")
        || text.contains("assert_ne!(")
        || text.contains("score_hit(")
        || text.contains("identifier_role(")
}

pub(super) fn is_sql_migration(uri: &str) -> bool {
    let lowercase_uri = uri.to_ascii_lowercase();
    lowercase_uri.contains("/migrations/") && lowercase_uri.contains(".sql")
}

pub(super) fn query_wants_schema(query: &str) -> bool {
    let lowercase_query = query.to_ascii_lowercase();
    lowercase_query.contains("schema")
        || lowercase_query.contains("migration")
        || lowercase_query.contains("column")
        || lowercase_query.contains("table")
        || lowercase_query.contains("sql")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tests_and_fixtures_rank_lowest() {
        let query = "What does should_skip_blob do?";
        assert_eq!(
            chunk_rank_tier(query, "src/tests/blob.rs", "fn should_skip_blob() {}"),
            0
        );
        assert_eq!(
            chunk_rank_tier(query, "evals/retrieval.json", "should_skip_blob"),
            0
        );
        assert_eq!(
            chunk_rank_tier(query, "src/blob.rs", "kind: file_header\nblob"),
            1
        );
        assert_eq!(
            chunk_rank_tier(query, "src/blob.rs", "fn should_skip_blob() {}"),
            3
        );
        assert_eq!(chunk_rank_tier(query, "src/blob.rs", "unrelated"), 2);
    }

    #[test]
    fn a_definition_holding_every_code_identifier_ranks_highest() {
        let query = "Why must retain_content_uris use live_uris?";
        assert_eq!(
            chunk_rank_tier(
                query,
                "src/store.rs",
                "fn retain_content_uris(live_uris: &[String]) {}"
            ),
            5
        );
        assert_eq!(
            chunk_rank_tier(query, "src/store.rs", "retain_content_uris(&live_uris)"),
            4
        );
    }

    #[test]
    fn a_prelude_has_no_implementation() {
        assert!(is_module_prelude("kind: top_level\n//! Store helpers."));
        assert!(!is_module_prelude("kind: top_level\npub fn run() {}"));
        assert!(!is_module_prelude(
            "kind: top_level\nCREATE TABLE things ()"
        ));
        assert!(!is_module_prelude("kind: function\n//! Store helpers."));
    }

    #[test]
    fn fixture_and_migration_uris_are_recognised() {
        assert!(is_fixture_uri(
            "github://acme/widgets/evals/cases.json@main"
        ));
        assert!(is_fixture_uri("runner/retrieval.json"));
        assert!(!is_fixture_uri("src/store.rs"));
        assert!(is_sql_migration("db/migrations/001_initial.sql"));
        assert!(!is_sql_migration("db/schema.sql"));
        assert!(query_wants_schema("Which columns exist?"));
        assert!(!query_wants_schema("How does login work?"));
    }
}
