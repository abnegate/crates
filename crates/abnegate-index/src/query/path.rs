//! File-name tokens and URI patterns that reach files first-stage search missed.

use std::collections::HashSet;

use crate::query::natural_language::content_tokens;
use crate::query::natural_language::keep_singleton;

const GENERIC_PATH_TOKENS: &[&str] = &[
    "knowledge",
    "embedding",
    "embeddings",
    "authorized",
    "authorizing",
    "configured",
    "constructed",
    "implemented",
    "incremental",
    "signature",
    "semantic",
    "encoder",
    "interval",
    "refresh",
    "worker",
    "process",
    "subject",
    "message",
    "content",
    "context",
    "schema",
    "unchanged",
    "oversized",
    "returned",
    "columns",
    "created",
    "generate",
    "validate",
    "dimension",
    "hybrid",
    "vector",
    "adapter",
    "webhook",
    "limiter",
];

/// File-name tokens of `query` worth fetching even when first-stage search
/// missed the file.
///
/// Each is at least eight ASCII letters, digits or underscores and is not a
/// generic word. A plural of nine or more bytes also yields its singular,
/// first.
pub fn path_uri_tokens(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut tokens = Vec::new();
    let mut push = |token: String| {
        if token.len() < 8
            || !token
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
            || !seen.insert(token.clone())
        {
            return;
        }
        tokens.push(token);
    };
    for token in content_tokens(query) {
        if !keep_singleton(&token) || is_generic_path_token(&token) {
            continue;
        }
        if token.ends_with('s') && !token.ends_with("ss") && token.len() >= 9 {
            push(token[..token.len() - 1].to_string());
        }
        push(token);
    }
    tokens
}

/// SQL `LIKE` patterns matching a URI whose file stem is one of
/// [`path_uri_tokens`]: `%/{token}.%` and `%/{token}@%` for each, stopping
/// once there are at least eight.
pub fn path_uri_patterns(query: &str) -> Vec<String> {
    let mut patterns = Vec::new();
    for token in path_uri_tokens(query) {
        patterns.push(format!("%/{token}.%"));
        patterns.push(format!("%/{token}@%"));
        if patterns.len() >= 8 {
            break;
        }
    }
    patterns
}

fn is_generic_path_token(token: &str) -> bool {
    GENERIC_PATH_TOKENS.contains(&token)
}
