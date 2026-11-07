//! The lexical cross-encoder and its blend with the learned ranker.

use crate::query::bridge_terms;
use crate::query::rewrite_query;
use crate::rank::identifier::identifier_role;
use crate::rank::role::IdentifierRole;

/// Blend a learned-ranker logit with a `0.0..=1.0` cross-encoder score:
/// `0.62 * learned + 0.38 * cross_encoder`.
pub fn blend_rank(learned: f32, cross_encoder: f32) -> f32 {
    0.62 * learned + 0.38 * cross_encoder
}

/// A joint query-document score in `0.0..=1.0`, used when no neural
/// cross-encoder is configured.
///
/// Sums the identifier role, identifier hits in the URI or title, query-token
/// coverage of the text, a whole-query phrase match, a token in the title or
/// URI, and snake-case bridge terms found in the text, then clamps.
pub fn lexical_cross_score(query: &str, uri: &str, title: &str, text: &str) -> f32 {
    let rewritten = rewrite_query(query);
    let lowercase_query = query.to_ascii_lowercase();
    let lowercase_uri = uri.to_ascii_lowercase();
    let lowercase_title = title.to_ascii_lowercase();
    let lowercase_text = text.to_ascii_lowercase();

    let role = identifier_role(text, uri, &rewritten.identifiers);
    let mut identifier_score: f32 = match role {
        IdentifierRole::Exported => 0.7,
        IdentifierRole::Defined => 0.55,
        IdentifierRole::Mention => 0.16,
        IdentifierRole::None => 0.0,
    };
    for identifier in &rewritten.identifiers {
        let lowercase_identifier = identifier.to_ascii_lowercase();
        if lowercase_uri.contains(&lowercase_identifier) {
            identifier_score += 0.12;
        } else if lowercase_title.contains(&lowercase_identifier) {
            identifier_score += 0.08;
        }
    }
    identifier_score = identifier_score.min(0.75);

    let tokens: Vec<&str> = query
        .split(|character: char| {
            !character.is_alphanumeric() && character != '_' && character != '-'
        })
        .filter(|token| token.len() >= 2)
        .collect();
    let coverage = if tokens.is_empty() {
        0.0
    } else {
        tokens
            .iter()
            .filter(|token| lowercase_text.contains(&token.to_ascii_lowercase()))
            .count() as f32
            / tokens.len() as f32
    };

    let phrase = if lowercase_query.len() >= 8 && lowercase_text.contains(&lowercase_query) {
        0.15
    } else {
        0.0
    };
    let title_hit = if tokens
        .iter()
        .any(|token| lowercase_title.contains(&token.to_ascii_lowercase()))
    {
        0.08
    } else {
        0.0
    };
    let path_hit = if tokens
        .iter()
        .any(|token| lowercase_uri.contains(&token.to_ascii_lowercase()))
    {
        0.07
    } else {
        0.0
    };

    let mut bridge = 0.0f32;
    for term in bridge_terms(query) {
        if term.contains('_') && lowercase_text.contains(&term) {
            bridge += 0.12;
        }
    }
    bridge = bridge.min(0.24);

    (identifier_score + 0.22 * coverage + phrase + title_hit + path_hit + bridge).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_cross_encoder_is_joint_over_query_and_document() {
        let query = "What does should_skip_blob do?";
        let exact = lexical_cross_score(
            query,
            "github://acme/widgets/content/mod.rs",
            "mod.rs",
            "fn should_skip_blob(uri: &str) -> bool",
        );
        let other = lexical_cross_score(
            query,
            "github://acme/widgets/auth.ts",
            "auth.ts",
            "login form",
        );
        assert!(exact > other);
        assert!(exact > 0.4);
    }

    #[test]
    fn blend_weights_the_learned_score_more() {
        assert_eq!(blend_rank(1.0, 0.0), 0.62);
        assert_eq!(blend_rank(0.0, 1.0), 0.38);
    }
}
