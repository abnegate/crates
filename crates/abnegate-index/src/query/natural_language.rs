//! Distinctive words, phrases and snake-case bridges from a plain-English question.

use std::collections::HashSet;

use crate::query::identifiers::extract_identifiers;
use crate::query::identifiers::tokenize;

const PAIR_ABBREVIATIONS: &[&str] = &[
    "cors", "jwt", "smtp", "mcp", "hmac", "aes", "gcm", "sha", "sql", "key",
];

const PAIR_SIDE_ABBREVIATIONS: &[&str] = &["key", "cors", "aes", "gcm", "jwt", "mcp", "sql"];

const PHRASE_SKIPS: &[&str] = &[
    "how", "does", "do", "did", "the", "a", "an", "is", "are", "was", "were", "when", "what",
    "where", "why", "who", "which", "for", "from", "with", "that", "this",
];

const REJECTED_SINGLETONS: &[&str] = &[
    "generated",
    "serving",
    "prevent",
    "unbounded",
    "growth",
    "often",
    "wake",
    "stored",
];

const SINGLETON_ABBREVIATIONS: &[&str] = &["cors", "jwt", "smtp", "mcp", "hmac", "aes", "gcm"];

const STOPWORDS: &[&str] = &[
    "how",
    "does",
    "do",
    "did",
    "the",
    "a",
    "an",
    "is",
    "are",
    "was",
    "were",
    "when",
    "what",
    "where",
    "why",
    "who",
    "which",
    "for",
    "from",
    "with",
    "that",
    "this",
    "into",
    "onto",
    "than",
    "then",
    "just",
    "used",
    "using",
    "after",
    "before",
    "about",
    "over",
    "under",
    "and",
    "or",
    "not",
    "can",
    "could",
    "should",
    "would",
    "will",
    "its",
    "their",
    "our",
    "must",
    "have",
    "has",
    "been",
    "being",
    "all",
    "any",
    "each",
    "return",
    "returned",
    "returns",
    "implement",
    "implemented",
    "please",
    "tell",
    "me",
    "on",
    "in",
    "at",
    "of",
    "to",
    "by",
    "up",
    "out",
    "off",
    "if",
    "as",
    "so",
    "no",
    "yes",
    "server",
    "client",
];

/// Distinctive question words and snake-case bridges to OR into keyword search.
///
/// English full-text search ANDs unquoted words, so a plain question misses
/// `verify_signature` unless its distinctive tokens are ORed together with
/// snake-case joins of adjacent words (`cors` + `origins` becomes
/// `cors_origins`) that already appear in indexed symbol and signature lines.
pub fn bridge_terms(query: &str) -> Vec<String> {
    expand_natural_language_terms(query)
}

/// Up to eight distinctive lowercase words of `query`, in order, used to
/// decide which chunk of a file answers the question.
///
/// Identifiers, stopwords, words with no letter, and short words other than
/// a few well-known acronyms are left out.
pub fn content_tokens(query: &str) -> Vec<String> {
    let identifiers: HashSet<String> = extract_identifiers(query)
        .into_iter()
        .map(|token| token.to_ascii_lowercase())
        .collect();
    let mut seen = HashSet::new();
    let mut tokens = Vec::new();
    for token in tokenize(query) {
        let token = token.to_ascii_lowercase();
        if identifiers.contains(&token)
            || is_stopword(&token)
            || !token
                .chars()
                .any(|character| character.is_ascii_alphabetic())
            || !keep_pair_token(&token)
            || !seen.insert(token.clone())
        {
            continue;
        }
        tokens.push(token);
        if tokens.len() == 8 {
            break;
        }
    }
    tokens
}

/// Adjacent content-word pairs from `query`, such as `not configured`.
///
/// Negations are kept, so the chunk that literally answers the question can be
/// preferred.
pub fn question_phrases(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for token in tokenize(query) {
        let token = token.to_ascii_lowercase();
        if is_phrase_skip(&token)
            || !token
                .chars()
                .any(|character| character.is_ascii_alphabetic())
        {
            continue;
        }
        tokens.push(token);
    }
    let mut seen = HashSet::new();
    let mut phrases = Vec::new();
    for pair in tokens.windows(2) {
        if !keep_pair_token(&pair[0]) && !keep_pair_token(&pair[1]) {
            continue;
        }
        if is_stopword(&pair[0]) && is_stopword(&pair[1]) {
            continue;
        }
        let phrase = format!("{} {}", pair[0], pair[1]);
        if phrase.len() >= 8 && seen.insert(phrase.clone()) {
            phrases.push(phrase);
        }
    }
    phrases
}

pub(super) fn expand_natural_language_terms(query: &str) -> Vec<String> {
    let tokens = content_tokens(query);
    let mut stems = Vec::new();
    for token in &tokens {
        if let Some(stem) = question_verb_stem(token)
            && stem != token.as_str()
        {
            stems.push(stem.to_string());
        }
    }
    let mut seen: HashSet<String> = tokens.iter().cloned().collect();
    let mut terms = Vec::new();
    for token in &tokens {
        if keep_singleton(token) {
            terms.push(token.clone());
        }
    }
    for stem in &stems {
        if keep_singleton(stem) && seen.insert(stem.clone()) {
            terms.push(stem.clone());
        }
    }
    let mut push_pair = |left: &str, right: &str| {
        if !allow_pair_side(left) || !allow_pair_side(right) {
            return;
        }
        let snake = format!("{left}_{right}");
        if snake.len() >= 8 && seen.insert(snake.clone()) {
            terms.push(snake);
        }
    };
    for pair in tokens.windows(2) {
        push_pair(&pair[0], &pair[1]);
    }
    if let (Some(first), Some(last)) = (tokens.first(), tokens.last())
        && first != last
    {
        push_pair(first, last);
    }
    for stem in &stems {
        for token in &tokens {
            if stem != token {
                push_pair(stem, token);
                push_pair(token, stem);
            }
        }
    }
    terms
}

fn question_verb_stem(token: &str) -> Option<&str> {
    Some(match token {
        "derived" | "derive" => "derive",
        "authorized" | "authorizing" => "authorize",
        "configured" => "configure",
        "trimmed" | "trim" => "trim",
        _ => return None,
    })
}

fn keep_pair_token(token: &str) -> bool {
    token.len() >= 4 || PAIR_ABBREVIATIONS.contains(&token)
}

fn allow_pair_side(token: &str) -> bool {
    token.len() >= 4 || PAIR_SIDE_ABBREVIATIONS.contains(&token)
}

fn is_phrase_skip(token: &str) -> bool {
    PHRASE_SKIPS.contains(&token)
}

pub(super) fn keep_singleton(token: &str) -> bool {
    if REJECTED_SINGLETONS.contains(&token) {
        return false;
    }
    token.len() >= 6 || SINGLETON_ABBREVIATIONS.contains(&token)
}

fn is_stopword(token: &str) -> bool {
    STOPWORDS.contains(&token)
}
