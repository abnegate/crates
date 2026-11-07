//! How much of a question a chunk's code, documentation and symbol cover.

use crate::hybrid::question::Question;
use crate::hybrid::uri::uri_file_stem;
use crate::rank::identifier_hit_count;

const SYMBOL_HEADER_LINES: usize = 24;

pub(super) fn conjunction_identifier_hit(text: &str, identifiers: &[String]) -> bool {
    let code_identifiers: Vec<&String> = identifiers
        .iter()
        .filter(|identifier| !identifier.contains('/'))
        .collect();
    code_identifiers.len() >= 2
        && code_identifiers
            .iter()
            .all(|identifier| text.contains(identifier.as_str()))
}

pub(super) fn exclusive_question_hits(question: &Question<'_>, text: &str, other: &str) -> u32 {
    let lowercase_text = text.to_ascii_lowercase();
    let lowercase_other = other.to_ascii_lowercase();
    let text_documentation = documentation_comment_text(text);
    let text_code = strip_documentation_comments(text);
    let other_code = strip_documentation_comments(other);
    let documentation_words = question
        .content_tokens
        .iter()
        .filter(|token| {
            text_documentation.contains(token.as_str()) && !lowercase_other.contains(token.as_str())
        })
        .count() as u32;
    let body_words = question
        .content_tokens
        .iter()
        .filter(|token| {
            lowercase_text.contains(token.as_str())
                && !text_documentation.contains(token.as_str())
                && !lowercase_other.contains(token.as_str())
        })
        .count() as u32;
    let phrases = question
        .phrases
        .iter()
        .filter(|phrase| {
            text_code.contains(phrase.as_str()) && !other_code.contains(phrase.as_str())
        })
        .count() as u32;
    documentation_words * 3 + body_words + phrases * 2
}

pub(super) fn question_coverage(question: &Question<'_>, text: &str) -> u32 {
    let identifier_hits = identifier_hit_count(text, &question.identifiers) as u32;
    let lowercase_text = text.to_ascii_lowercase();
    let bridges = count_contained(&question.bridge_terms, &lowercase_text);
    let words = count_contained(&question.content_tokens, &lowercase_text);
    let phrases = count_contained(&question.phrases, &lowercase_text);
    let symbol = defined_symbol_overlap(question, text);
    let documentation = if function_kind(text) {
        documentation_comment_coverage(question, text)
    } else {
        0
    };
    identifier_hits * 2 + bridges + words + phrases * 3 + symbol * 3 + documentation * 2
}

pub(super) fn defined_symbol_prefix(question: &Question<'_>, text: &str) -> u32 {
    if !function_kind(text) {
        return 0;
    }
    let Some(leaf) = defined_leaf_symbol(text) else {
        return 0;
    };
    u32::from(
        question
            .tokens_and_stems()
            .any(|token| leaf.starts_with(token) || (token.len() >= 6 && leaf.contains(token))),
    )
}

pub(super) fn filename_stem_hits(uri: &str, text: &str) -> u32 {
    let Some(stem) = uri_file_stem(uri) else {
        return 0;
    };
    let mut stems = vec![stem.clone()];
    if stem.ends_with('s') && !stem.ends_with("ss") && stem.len() >= 5 {
        stems.push(stem[..stem.len() - 1].to_string());
    }
    identifier_like_tokens(text)
        .into_iter()
        .filter(|token| {
            let lowercase_token = token.to_ascii_lowercase();
            stems.iter().any(|stem| {
                lowercase_token.starts_with(stem.as_str())
                    && lowercase_token.len() > stem.len()
                    && (token
                        .chars()
                        .any(|character| character.is_ascii_uppercase())
                        || token.contains('_'))
            })
        })
        .count() as u32
}

pub(super) fn defined_leaf_symbol(text: &str) -> Option<String> {
    for line in text.lines().take(SYMBOL_HEADER_LINES) {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix("symbol: ")
            .or_else(|| line.strip_prefix("Symbol: "))
        else {
            continue;
        };
        let Some(leaf) = rest.split('.').next_back().map(str::trim) else {
            continue;
        };
        if leaf.len() >= 3 {
            return Some(leaf.to_ascii_lowercase());
        }
    }
    None
}

fn count_contained(terms: &[String], lowercase_text: &str) -> u32 {
    terms
        .iter()
        .filter(|term| lowercase_text.contains(term.as_str()))
        .count() as u32
}

fn documentation_comment_coverage(question: &Question<'_>, text: &str) -> u32 {
    let comments = documentation_comment_lines(text)
        .join(" ")
        .to_ascii_lowercase();
    if comments.is_empty() {
        return 0;
    }
    count_contained(&question.content_tokens, &comments)
}

fn defined_symbol_overlap(question: &Question<'_>, text: &str) -> u32 {
    if !function_kind(text) {
        return 0;
    }
    let Some(leaf) = defined_leaf_symbol(text) else {
        return 0;
    };
    u32::from(
        question.tokens_and_stems().any(|token| {
            leaf.contains(token) || (token.len() >= 4 && token.contains(leaf.as_str()))
        }),
    )
}

fn function_kind(text: &str) -> bool {
    if text.lines().take(SYMBOL_HEADER_LINES).any(|line| {
        let line = line.trim();
        line.eq_ignore_ascii_case("kind: Function")
            || line.eq_ignore_ascii_case("kind: Method")
            || line.eq_ignore_ascii_case("kind: Assoc")
    }) {
        return true;
    }
    let Some(leaf) = defined_leaf_symbol(text) else {
        return false;
    };
    text.contains(&format!("fn {leaf}(")) || text.contains(&format!("fn {leaf} ("))
}

fn is_documentation_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("///") || trimmed.starts_with("//!")
}

fn documentation_comment_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| is_documentation_comment(line))
        .collect()
}

fn documentation_comment_text(text: &str) -> String {
    documentation_comment_lines(text)
        .join("\n")
        .to_ascii_lowercase()
}

fn strip_documentation_comments(text: &str) -> String {
    text.lines()
        .filter(|line| !is_documentation_comment(line))
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase()
}

fn identifier_like_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_leaf_symbol_is_the_last_dotted_segment() {
        assert_eq!(
            defined_leaf_symbol("symbol: RateLimiter.cleanup\nkind: Method"),
            Some("cleanup".into())
        );
        assert_eq!(defined_leaf_symbol("symbol: ab\n"), None);
    }

    #[test]
    fn stem_hits_count_compound_identifiers_named_for_the_file() {
        assert_eq!(
            filename_stem_hits(
                "github://acme/widgets/src/artifacts.rs@main",
                "let store = ArtifactStore::new(); let artifact_root = 1; artifacts"
            ),
            2
        );
    }

    #[test]
    fn documentation_comments_split_from_code() {
        let text = "/// Clean up\n//! Module\nfn cleanup() {}";
        assert_eq!(documentation_comment_text(text), "/// clean up\n//! module");
        assert_eq!(strip_documentation_comments(text), "fn cleanup() {}");
    }
}
