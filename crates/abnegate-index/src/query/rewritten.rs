//! A question paired with the identifier-focused query used for full-text search.

use crate::query::identifiers::expand_keyword_identifiers;
use crate::query::identifiers::extract_identifiers;
use crate::query::natural_language::expand_natural_language_terms;

/// A question paired with the identifier-focused query used for full-text
/// search.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RewrittenQuery {
    /// The question as asked.
    pub original: String,
    /// Terms joined with ` OR ` for PostgreSQL `websearch_to_tsquery`.
    pub keyword: String,
    /// Paths, Rust paths, and snake, kebab and camel case identifiers found in
    /// the question, in first-seen order.
    pub identifiers: Vec<String>,
}

impl RewrittenQuery {
    /// A rewritten `original` with its keyword query and identifiers.
    pub fn new(
        original: impl Into<String>,
        keyword: impl Into<String>,
        identifiers: Vec<String>,
    ) -> Self {
        Self {
            original: original.into(),
            keyword: keyword.into(),
            identifiers,
        }
    }
}

/// Extract symbols, paths and code-like tokens from `query` so keyword search
/// is not English-stemmed into mush: `should_skip_blob` must not become
/// `shouldskipblob`.
///
/// Identifiers stay intact and are ORed, so a path plus a symbol does not
/// require both in one chunk. Distinctive question words and their snake-case
/// bridges are ORed after them.
pub fn rewrite_query(query: &str) -> RewrittenQuery {
    let identifiers = extract_identifiers(query);
    let natural_language_terms = expand_natural_language_terms(query);
    let keyword = if identifiers.is_empty() {
        if natural_language_terms.is_empty() {
            full_text_tokens(query)
        } else {
            natural_language_terms.join(" OR ")
        }
    } else {
        let identifier_terms = expand_keyword_identifiers(&identifiers);
        let extra: Vec<String> = natural_language_terms
            .into_iter()
            .filter(|term| {
                !identifier_terms
                    .split_whitespace()
                    .any(|part| part.eq_ignore_ascii_case(term))
            })
            .collect();
        if extra.is_empty() {
            identifier_terms
        } else {
            format!("{identifier_terms} OR {}", extra.join(" OR "))
        }
    };
    RewrittenQuery {
        original: query.to_string(),
        keyword,
        identifiers,
    }
}

fn full_text_tokens(query: &str) -> String {
    query
        .split(|character: char| {
            !(character.is_alphanumeric() || character == '_' || character == '-')
        })
        .filter(|part| part.len() >= 2)
        .map(|part| part.replace('_', " "))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::bridge_terms;
    use crate::query::path_uri_patterns;
    use crate::query::path_uri_tokens;
    use crate::query::question_phrases;

    #[test]
    fn extracts_snake_and_path() {
        let rewritten = rewrite_query(
            "What does should_skip_blob do when a GitHub file SHA is unchanged in content/mod.rs?",
        );
        assert!(
            rewritten
                .identifiers
                .iter()
                .any(|identifier| identifier == "should_skip_blob")
        );
        assert!(
            rewritten
                .identifiers
                .iter()
                .any(|identifier| identifier.contains("content/mod.rs"))
        );
        assert!(rewritten.keyword.contains("should_skip_blob"));
        assert!(
            !rewritten
                .identifiers
                .iter()
                .any(|identifier| identifier == "GitHub"),
            "proper nouns are not type identifiers: {:?}",
            rewritten.identifiers
        );
        assert!(rewritten.keyword.contains("OR") || rewritten.keyword == "should_skip_blob");
    }

    #[test]
    fn extracts_kebab_and_api_path() {
        let rewritten = rewrite_query(
            "What budget does embed_char_budget return for nomic-embed-text at /api/embeddings?",
        );
        assert!(
            rewritten
                .identifiers
                .iter()
                .any(|identifier| identifier == "nomic-embed-text")
        );
        assert!(
            rewritten
                .identifiers
                .iter()
                .any(|identifier| identifier == "embed_char_budget")
        );
        assert!(rewritten.identifiers.iter().any(|identifier| {
            identifier == "/api/embeddings" || identifier.contains("api/embeddings")
        }));
    }

    #[test]
    fn extracts_camel_case() {
        let rewritten = rewrite_query("Where is GitHubAdapter constructed?");
        assert!(
            rewritten
                .identifiers
                .iter()
                .any(|identifier| identifier == "GitHubAdapter")
        );
    }

    #[test]
    fn keyword_expands_request_types_and_environment_constants() {
        let request = rewrite_query("What fields does CreateKnowledgeRequest require?");
        assert!(request.keyword.contains("CreateKnowledgeRequest"));
        assert!(request.keyword.contains("create_knowledge"));
        assert!(
            request
                .identifiers
                .iter()
                .all(|identifier| identifier != "create_knowledge")
        );

        let environment = rewrite_query("What is SOURCE_RESYNC_POLL_SECS used for?");
        assert!(environment.keyword.contains("SOURCE_RESYNC_POLL_SECS"));
        assert!(environment.keyword.contains("source_resync_poll_secs"));
        assert!(environment.keyword.contains("resync"));
        assert!(
            !environment
                .keyword
                .split_whitespace()
                .any(|term| term == "source")
        );
    }

    #[test]
    fn natural_language_query_ors_distinctive_tokens_and_snake_bridges() {
        let webhook = rewrite_query("How does the server verify a GitHub webhook signature?");
        assert!(webhook.identifiers.is_empty());
        assert!(webhook.keyword.contains(" OR "));
        assert!(webhook.keyword.contains("webhook"));
        assert!(webhook.keyword.contains("signature"));
        assert!(
            webhook.keyword.contains("webhook_signature")
                || webhook.keyword.contains("verify_signature")
        );

        let cors = rewrite_query("When does the API allow all CORS origins?");
        assert!(cors.keyword.to_ascii_lowercase().contains("cors"));
        assert!(cors.keyword.contains("cors_origins"));

        let rate = rewrite_query("How does the rate limiter prevent unbounded memory growth?");
        assert!(rate.keyword.contains("rate_limiter"));

        let refresh = rewrite_query("How often does the knowledge refresh worker wake up?");
        assert!(refresh.keyword.contains("knowledge_refresh"));

        let email = rewrite_query("What error is returned when SMTP is not configured?");
        assert!(email.keyword.contains("smtp"));
        assert!(
            !email.keyword.split(" OR ").any(|term| term == "error"),
            "bare 'error' floods error.rs: {}",
            email.keyword
        );
        assert!(
            question_phrases("What error is returned when SMTP is not configured?")
                .iter()
                .any(|phrase| phrase == "not configured")
        );

        let derive = rewrite_query("Where is the AES-256 encryption key derived?");
        assert!(
            derive.keyword.split(" OR ").any(|term| term == "derive"),
            "the stem names secret::derive: {}",
            derive.keyword
        );

        let artifact = "How are generated chat artifacts authorized before serving?";
        assert!(
            path_uri_tokens(artifact)
                .iter()
                .any(|token| token == "artifact" || token == "artifacts"),
            "path tokens {:?}",
            path_uri_tokens(artifact)
        );
        let patterns = path_uri_patterns(artifact);
        assert!(
            patterns.iter().any(|pattern| {
                let uri = "github://acme/widgets/routes/artifacts.rs@main";
                let lowercase_uri = uri.to_ascii_lowercase();
                let needle = pattern.trim_start_matches('%').trim_end_matches('%');
                lowercase_uri.contains(needle)
            }),
            "patterns {patterns:?}"
        );
        assert!(
            path_uri_tokens("How often does the knowledge refresh worker wake up?").is_empty(),
            "knowledge is too generic to path-expand"
        );
        assert!(
            path_uri_tokens(
                "Which columns does 001_initial_schema create on the embeddings table?"
            )
            .is_empty(),
            "embeddings is too generic to path-expand"
        );
        let live = rewrite_query(
            "Why must retain_content_uris use live_uris after an incremental gather?",
        );
        assert!(
            live.identifiers
                .iter()
                .any(|identifier| identifier == "retain_content_uris")
        );
        assert!(
            live.identifiers
                .iter()
                .any(|identifier| identifier == "live_uris")
        );

        let identifier =
            rewrite_query("What does should_skip_blob do when a file SHA is unchanged?");
        assert!(identifier.keyword.contains("should_skip_blob"));
        assert!(identifier.keyword.contains("unchanged"));
        assert!(!identifier.keyword.contains("what_does"));
        assert!(
            bridge_terms(&identifier.original)
                .iter()
                .any(|term| term == "unchanged")
        );
    }

    #[test]
    fn new_fills_every_field() {
        let rewritten = RewrittenQuery::new("a b", "a OR b", vec!["a_b".to_string()]);
        assert_eq!(rewritten.original, "a b");
        assert_eq!(rewritten.keyword, "a OR b");
        assert_eq!(rewritten.identifiers, ["a_b"]);
    }
}
