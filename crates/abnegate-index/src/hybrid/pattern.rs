//! `ILIKE` patterns that pick a fused file's other relevant chunks.

use std::collections::HashSet;

use crate::query::bridge_terms;
use crate::query::content_tokens;
use crate::query::rewrite_query;

const MAXIMUM_SAME_FILE_PATTERNS: usize = 16;

/// `%term%` `ILIKE` patterns for the question's identifiers, content words and
/// snake-case bridges, in that order, for ranking a matched file's other
/// chunks.
///
/// Terms are lowercased, must be at least three ASCII letters, digits or
/// underscores, appear once, and stop at sixteen patterns.
pub fn same_file_patterns(query: &str) -> Vec<String> {
    let identifiers = rewrite_query(query).identifiers;
    let mut seen = HashSet::new();
    let mut patterns = Vec::new();
    let mut push = |token: &str| {
        let token = token.to_ascii_lowercase();
        if token.len() < 3
            || !token
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
            || !seen.insert(token.clone())
        {
            return;
        }
        patterns.push(format!("%{token}%"));
    };
    for identifier in &identifiers {
        push(identifier);
    }
    for token in content_tokens(query) {
        push(&token);
    }
    for token in bridge_terms(query) {
        push(&token);
    }
    patterns.truncate(MAXIMUM_SAME_FILE_PATTERNS);
    patterns
}

/// `%identifier%` `ILIKE` patterns for the question's code identifiers of four
/// or more characters, skipping paths, with the identifier's case kept and
/// case-insensitive duplicates dropped.
pub fn identifier_file_patterns(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut patterns = Vec::new();
    for identifier in rewrite_query(query).identifiers {
        if identifier.len() < 4
            || identifier.contains('/')
            || !seen.insert(identifier.to_ascii_lowercase())
        {
            continue;
        }
        patterns.push(format!("%{identifier}%"));
    }
    patterns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_patterns_skip_paths_and_short_identifiers() {
        let patterns =
            identifier_file_patterns("Where does src/inventory/warehouse.rs call reserve_stock?");
        assert!(patterns.contains(&"%reserve_stock%".to_string()));
        assert!(patterns.iter().all(|pattern| !pattern.contains('/')));
    }

    #[test]
    fn same_file_patterns_start_with_identifiers() {
        let patterns = same_file_patterns("What does reserve_stock do when the bin is locked?");
        assert_eq!(
            patterns.first().map(String::as_str),
            Some("%reserve_stock%")
        );
        assert!(patterns.len() <= MAXIMUM_SAME_FILE_PATTERNS);
    }
}
