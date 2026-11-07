//! Whether a chunk is the real definition a question asks about.

use crate::query::content_tokens;
use crate::query::rewrite_query;
use crate::rank::chunk::is_outline_chunk;
use crate::rank::chunk::is_sql_migration;
use crate::rank::chunk::is_test_chunk;
use crate::rank::identifier::identifier_role;
use crate::rank::identifier::tool_name_definition;

/// Whether the first keyword hit already defines what `query` asks about, so
/// embedding and neural reranking can be skipped.
///
/// `hits` yields `(uri, text)` pairs in rank order; only the first is judged.
/// It must not be a test or outline, must define a code identifier, must not
/// be a bare tool name that misses the question, and must contain every code
/// identifier when the question names two or more.
pub fn first_stage_answered<'a>(
    query: &str,
    hits: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> bool {
    let identifiers = rewrite_query(query).identifiers;
    if identifiers.is_empty() {
        return false;
    }
    hits.into_iter().next().is_some_and(|(uri, text)| {
        !is_test_chunk(uri, text)
            && !is_outline_chunk(text)
            && code_definition(text, uri, &identifiers)
            && !(name_only_definition(text, &identifiers)
                && definition_misses_question(query, text))
            && conjunction_satisfied(text, &identifiers)
    })
}

/// Whether the chunk defines an identifier from `query` and does not miss the
/// rest of the question.
pub fn answers_as_definition(query: &str, uri: &str, text: &str) -> bool {
    let identifiers = rewrite_query(query).identifiers;
    if identifiers.is_empty() {
        return false;
    }
    identifier_role(text, uri, &identifiers).is_definition()
        && !definition_misses_question(query, text)
}

/// Whether two or more of the question's content words that are not part of an
/// identifier exist and none of them appears in `text`.
pub fn definition_misses_question(query: &str, text: &str) -> bool {
    let identifiers = rewrite_query(query).identifiers;
    let lowercase_text = text.to_ascii_lowercase();
    let extras: Vec<String> = content_tokens(query)
        .into_iter()
        .filter(|token| {
            !identifiers.iter().any(|identifier| {
                let lowercase_identifier = identifier.to_ascii_lowercase();
                lowercase_identifier.contains(token) || token.contains(&lowercase_identifier)
            })
        })
        .collect();
    extras.len() >= 2 && extras.iter().all(|token| !lowercase_text.contains(token))
}

pub(super) fn code_definition(text: &str, uri: &str, identifiers: &[String]) -> bool {
    if is_sql_migration(uri) {
        let lowercase_uri = uri.to_ascii_lowercase();
        let names_file = identifiers.iter().any(|identifier| {
            let lowercase_identifier = identifier.to_ascii_lowercase();
            lowercase_identifier.len() >= 8 && lowercase_uri.contains(&lowercase_identifier)
        });
        if !names_file {
            return false;
        }
    }
    let code_identifiers: Vec<String> = identifiers
        .iter()
        .filter(|identifier| !identifier.contains('/'))
        .cloned()
        .collect();
    if code_identifiers.is_empty() {
        return identifier_role(text, uri, identifiers).is_definition();
    }
    identifier_role(text, uri, &code_identifiers).is_definition()
}

fn conjunction_satisfied(text: &str, identifiers: &[String]) -> bool {
    let code_identifiers: Vec<&String> = identifiers
        .iter()
        .filter(|identifier| !identifier.contains('/'))
        .collect();
    code_identifiers.len() < 2
        || code_identifiers
            .iter()
            .all(|identifier| text.contains(identifier.as_str()))
}

fn name_only_definition(text: &str, identifiers: &[String]) -> bool {
    identifiers
        .iter()
        .any(|identifier| tool_name_definition(text, identifier))
        && identifiers.iter().all(|identifier| {
            !text.contains(&format!("fn {identifier}("))
                && !text.contains(&format!("fn {identifier} ("))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_stage_answered_needs_a_real_definition() {
        let query = "What does should_skip_blob do when a GitHub file SHA is unchanged?";
        let definition = (
            "github://acme/widgets/content/mod.rs@main",
            "impl FetchConfig { pub fn should_skip_blob(&self, file_uri: &str, blob_sha: &str) -> bool { true } }",
        );
        let mention = (
            "github://acme/widgets/embeddings/query.rs@main",
            "assert!(rewritten.identifiers.iter().any(|i| i == \"should_skip_blob\"));",
        );
        let test = (
            "github://acme/widgets/embeddings/ranker.rs@main",
            "#[cfg(test)]\nfn quoted_tool() { let _ = \"pub fn should_skip_blob\"; }",
        );
        assert!(first_stage_answered(query, [definition]));
        assert!(first_stage_answered(query, [definition, mention]));
        assert!(!first_stage_answered(query, [mention, definition]));
        assert!(!first_stage_answered(query, [mention]));
        assert!(!first_stage_answered(query, [test, mention]));
        assert!(!first_stage_answered(
            "How does the server verify a webhook signature?",
            [definition],
        ));
        assert!(!first_stage_answered(
            "Where is search_messages implemented?",
            [(
                "github://acme/widgets/routes/chats.rs@main",
                "GET /api/chats/search | search_messages",
            )],
        ));
        assert!(!first_stage_answered(
            "How does run_command trim oversized stdout?",
            [(
                "github://acme/widgets/tools/command.rs@main",
                "fn name(&self) -> &str {\n        \"run_command\"\n    }",
            )],
        ));
        assert!(
            !first_stage_answered(
                "Why must retain_content_uris use live_uris after an incremental gather?",
                [(
                    "github://acme/widgets/embeddings/pgvector.rs@main",
                    "pub async fn retain_content_uris(&self, source_id: Uuid, uris: &[String]) { }",
                )],
            ),
            "one identifier is not enough when the question names two"
        );
        let execute = concat!(
            "symbol: RunCommandTool.execute\nkind: Method\nParent: RunCommandTool\n",
            "async fn execute(&self, params: Value, context: &ToolContext) {\n",
            "    cmd.stdout(Stdio::piped());\n",
            "}\n",
        );
        assert!(
            !first_stage_answered(
                "How does run_command trim oversized stdout?",
                [(
                    "github://acme/widgets/runner/core/src/tools/command.rs@main",
                    execute,
                )],
            ),
            "execute is not a run_command definition"
        );
        assert!(
            !answers_as_definition(
                "How does run_command trim oversized stdout?",
                "github://acme/widgets/runner/core/src/tools/command.rs@main",
                execute,
            ),
            "execute should not win the file row as a definition"
        );
    }

    #[test]
    fn a_definition_that_names_none_of_the_question_misses_it() {
        let query = "How does run_command trim oversized stdout?";
        assert!(definition_misses_question(query, "fn run_command() {}"));
        assert!(!definition_misses_question(
            query,
            "fn run_command() { trim(stdout) }"
        ));
    }
}
