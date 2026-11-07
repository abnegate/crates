//! Finding code identifiers in a question and widening them for keyword search.

use std::collections::HashSet;

const GENERIC_IDENTIFIER_PARTS: &[&str] = &[
    "source", "target", "config", "default", "value", "index", "server", "client", "worker",
    "handle", "result", "error", "request", "state", "embed", "query",
];

const REQUEST_SUFFIXES: &[&str] = &["Request", "Response", "Error", "Params", "Options", "Args"];

pub(super) fn extract_identifiers(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    let mut push = |token: String| {
        if token.len() < 2 || !seen.insert(token.clone()) {
            return;
        }
        output.push(token);
    };

    for token in tokenize(query) {
        if is_path(&token)
            || is_rust_path(&token)
            || is_snake(&token)
            || is_kebab(&token)
            || is_camel(&token)
        {
            push(token);
        }
    }
    output
}

pub(super) fn tokenize(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in query.chars() {
        if character.is_alphanumeric() || matches!(character, '_' | '.' | '/' | ':' | '-') {
            current.push(character);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn is_path(token: &str) -> bool {
    token.contains('/')
}

fn is_rust_path(token: &str) -> bool {
    token.contains("::")
}

fn is_snake(token: &str) -> bool {
    token.contains('_')
        && token
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
        && token
            .chars()
            .any(|character| character.is_ascii_alphabetic())
}

fn is_kebab(token: &str) -> bool {
    token.contains('-')
        && !token.starts_with('-')
        && !token.ends_with('-')
        && token
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
        && token
            .chars()
            .any(|character| character.is_ascii_alphabetic())
}

fn is_camel(token: &str) -> bool {
    if token.contains('_')
        || token.contains('-')
        || token.contains('/')
        || token.contains(':')
        || token.contains('.')
    {
        return false;
    }
    let mut lower = false;
    let mut humps = 0u32;
    for character in token.chars() {
        if character.is_ascii_lowercase() {
            lower = true;
        } else if character.is_ascii_uppercase() && lower {
            humps += 1;
            lower = false;
        }
    }
    (humps >= 2 || (humps >= 1 && token.len() >= 10))
        && token
            .chars()
            .any(|character| character.is_ascii_alphabetic())
}

pub(super) fn expand_keyword_identifiers(identifiers: &[String]) -> String {
    let mut seen = HashSet::new();
    let mut terms = Vec::new();
    let mut push = |token: String| {
        if token.len() < 2 || !seen.insert(token.clone()) {
            return;
        }
        terms.push(token);
    };
    for identifier in identifiers {
        push(identifier.clone());
        if is_screaming_snake(identifier) {
            push(identifier.to_ascii_lowercase());
            for part in identifier.split('_') {
                if part.len() >= 5 && !is_generic_identifier_part(part) {
                    push(part.to_ascii_lowercase());
                }
            }
        }
        if let Some(snake) = request_style_snake(identifier) {
            push(snake);
        }
    }
    terms.join(" OR ")
}

fn is_screaming_snake(token: &str) -> bool {
    token.contains('_')
        && token
            .chars()
            .any(|character| character.is_ascii_alphabetic())
        && token.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
}

fn is_generic_identifier_part(part: &str) -> bool {
    GENERIC_IDENTIFIER_PARTS.contains(&part.to_ascii_lowercase().as_str())
}

fn request_style_snake(token: &str) -> Option<String> {
    for suffix in REQUEST_SUFFIXES {
        if let Some(prefix) = token.strip_suffix(suffix)
            && let Some(snake) = camel_to_snake(prefix)
            && snake.contains('_')
        {
            return Some(snake);
        }
    }
    None
}

fn camel_to_snake(token: &str) -> Option<String> {
    if token.is_empty() {
        return None;
    }
    let mut output = String::new();
    for (index, character) in token.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                output.push('_');
            }
            output.push(character.to_ascii_lowercase());
        } else {
            output.push(character);
        }
    }
    Some(output)
}
