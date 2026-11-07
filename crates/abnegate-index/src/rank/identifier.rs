//! Whether a chunk defines, exports or merely mentions a question's identifiers.

use crate::rank::chunk::is_test_chunk;
use crate::rank::chunk::looks_like_unit_test_body;
use crate::rank::role::IdentifierRole;

const TOOL_NAME_WINDOW: usize = 160;

/// The strongest role any of `identifiers` plays in a chunk.
///
/// A test chunk is demoted from a definition to a
/// [`Mention`](IdentifierRole::Mention), because indexed unit tests copy
/// `fn name()` examples that would otherwise outrank the real implementation.
pub fn identifier_role(text: &str, uri: &str, identifiers: &[String]) -> IdentifierRole {
    if identifiers.is_empty() {
        return IdentifierRole::None;
    }
    let lowercase_uri = uri.to_ascii_lowercase();
    let mut best = IdentifierRole::None;
    for identifier in identifiers {
        best = strongest_role(best, role_for_identifier(text, &lowercase_uri, identifier));
    }
    if is_test_chunk(uri, text) && best.is_definition() {
        return IdentifierRole::Mention;
    }
    best
}

/// [`identifier_role`] as a number: 3 exported, 2 defined, 1 mention, 0 none.
pub fn role_strength(text: &str, uri: &str, identifiers: &[String]) -> u8 {
    match identifier_role(text, uri, identifiers) {
        IdentifierRole::Exported => 3,
        IdentifierRole::Defined => 2,
        IdentifierRole::Mention => 1,
        IdentifierRole::None => 0,
    }
}

/// How many of `identifiers` appear verbatim in `text`.
pub fn identifier_hit_count(text: &str, identifiers: &[String]) -> usize {
    identifiers
        .iter()
        .filter(|identifier| text.contains(identifier.as_str()))
        .count()
}

/// The share of underscore, hyphen and slash parts of `identifiers`, four
/// bytes or longer, that appear in `text` ignoring ASCII case; `0.0` when
/// there are no such parts.
pub fn identifier_part_coverage(text: &str, identifiers: &[String]) -> f32 {
    let lowercase_text = text.to_ascii_lowercase();
    let mut parts = Vec::new();
    for identifier in identifiers {
        for part in identifier.split(['_', '-', '/']) {
            if part.len() >= 4 {
                parts.push(part.to_ascii_lowercase());
            }
        }
    }
    if parts.is_empty() {
        return 0.0;
    }
    parts
        .iter()
        .filter(|part| lowercase_text.contains(*part))
        .count() as f32
        / parts.len() as f32
}

pub(super) fn tool_name_definition(text: &str, identifier: &str) -> bool {
    let quoted = format!("\"{identifier}\"");
    let mut start = 0;
    while let Some(relative) = text[start..].find("fn name(") {
        let at = start + relative;
        if is_inside_quotes(text, at) {
            start = at + 8;
            continue;
        }
        let end = text.floor_char_boundary((at + TOOL_NAME_WINDOW).min(text.len()));
        if text[at..end].contains(&quoted) {
            return true;
        }
        start = at + 8;
    }
    false
}

fn strongest_role(left: IdentifierRole, right: IdentifierRole) -> IdentifierRole {
    match (left, right) {
        (IdentifierRole::Exported, _) | (_, IdentifierRole::Exported) => IdentifierRole::Exported,
        (IdentifierRole::Defined, _) | (_, IdentifierRole::Defined) => IdentifierRole::Defined,
        (IdentifierRole::Mention, _) | (_, IdentifierRole::Mention) => IdentifierRole::Mention,
        _ => IdentifierRole::None,
    }
}

fn identifier_form(text: &str, form: &str) -> bool {
    let mut start = 0;
    while let Some(offset) = text[start..].find(form) {
        let at = start + offset;
        let after = text[at + form.len()..].chars().next();
        if !matches!(after, Some(character) if character.is_ascii_alphanumeric() || character == '_')
            && !is_inside_quotes(text, at)
        {
            return true;
        }
        start = at + form.len();
    }
    false
}

fn is_inside_quotes(text: &str, at: usize) -> bool {
    let line_start = text[..at].rfind('\n').map(|index| index + 1).unwrap_or(0);
    text[line_start..at]
        .chars()
        .filter(|&character| character == '"')
        .count()
        % 2
        == 1
}

fn role_for_identifier(text: &str, lowercase_uri: &str, identifier: &str) -> IdentifierRole {
    if identifier.is_empty() {
        return IdentifierRole::None;
    }
    if identifier_form(text, &format!("pub fn {identifier}"))
        || identifier_form(text, &format!("pub async fn {identifier}"))
        || identifier_form(text, &format!("pub struct {identifier}"))
        || identifier_form(text, &format!("pub enum {identifier}"))
        || identifier_form(text, &format!("pub const {identifier}"))
        || identifier_form(text, &format!("pub static {identifier}"))
        || identifier_form(text, &format!("pub type {identifier}"))
    {
        return IdentifierRole::Exported;
    }
    if identifier_form(text, &format!("pub(crate) fn {identifier}"))
        || identifier_form(text, &format!("fn {identifier}"))
        || identifier_form(text, &format!("async fn {identifier}"))
        || identifier_form(text, &format!("struct {identifier}"))
        || identifier_form(text, &format!("enum {identifier}"))
        || identifier_form(text, &format!("type {identifier}"))
        || identifier_form(text, &format!("const {identifier}"))
        || identifier_form(text, &format!("static {identifier}"))
        || identifier_form(text, &format!("CREATE TABLE {identifier}"))
        || identifier_form(text, &format!("CREATE TABLE IF NOT EXISTS {identifier}"))
        || identifier_form(text, &format!("CREATE OR REPLACE FUNCTION {identifier}"))
        || text.contains(&format!("name: \"{identifier}\""))
        || text.contains(&format!("\"{identifier}\" =>"))
        || text.contains(&format!("'{identifier}' =>"))
        || text.contains(&format!("env_u64(\"{identifier}\""))
        || text.contains(&format!("env::var(\"{identifier}\""))
        || tool_name_definition(text, identifier)
        || identifier_form(text, &format!("pub {identifier}:"))
        || identifier_form(text, &format!("pub(crate) {identifier}:"))
        || reexport_definition(text, identifier)
        || enum_variant_definition(text, identifier)
    {
        return IdentifierRole::Defined;
    }
    if identifier.contains('/')
        && !looks_like_unit_test_body(text)
        && (text.contains(&format!("\"{identifier}\""))
            || text.contains(&format!("'{identifier}'"))
            || (text.contains(identifier)
                && (text.contains("format!(")
                    || text.contains(".post(")
                    || text.contains("path(\""))))
    {
        return IdentifierRole::Defined;
    }
    let file = lowercase_uri
        .rsplit('/')
        .next()
        .unwrap_or(lowercase_uri)
        .split('@')
        .next()
        .unwrap_or(lowercase_uri);
    let stem = file.split('.').next().unwrap_or(file);
    let lowercase_identifier = identifier.to_ascii_lowercase();
    if stem == lowercase_identifier
        || (lowercase_identifier.len() >= 8 && stem.contains(&lowercase_identifier))
    {
        return IdentifierRole::Defined;
    }
    if text.contains(identifier) || lowercase_uri.contains(&lowercase_identifier) {
        return IdentifierRole::Mention;
    }
    IdentifierRole::None
}

fn reexport_definition(text: &str, identifier: &str) -> bool {
    text.contains("pub use ")
        && (text.contains(&format!("::{identifier}"))
            || text.contains(&format!(" {identifier};"))
            || text.contains(&format!("::{identifier};")))
}

fn enum_variant_definition(text: &str, identifier: &str) -> bool {
    if !text.contains("enum ") || identifier.len() < 4 {
        return false;
    }
    text.contains(&format!("\n    {identifier},"))
        || text.contains(&format!("\n    {identifier}\n"))
        || text.contains(&format!("\n        {identifier},"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_tool_and_sql_stem_count_as_definitions() {
        assert_eq!(
            identifier_role(
                r#"fn name(&self) -> &str { "search_knowledge" }"#,
                "agent/tools.rs",
                &["search_knowledge".into()],
            ),
            IdentifierRole::Defined
        );
        assert_eq!(
            identifier_role(
                "CREATE TABLE IF NOT EXISTS embeddings (id UUID)",
                "migrations/001_initial_schema.sql",
                &["001_initial_schema".into()],
            ),
            IdentifierRole::Defined
        );
        assert_eq!(
            identifier_role(
                r#"let url = format!("{}/api/embeddings", self.base_url);"#,
                "providers/ollama.rs",
                &["/api/embeddings".into()],
            ),
            IdentifierRole::Defined
        );
        assert_eq!(
            identifier_role(
                r#"assert!(rewritten.identifiers.iter().any(|i| i == "should_skip_blob"));"#,
                "embeddings/query.rs",
                &["should_skip_blob".into()],
            ),
            IdentifierRole::Mention
        );
        assert_eq!(
            identifier_role(
                "pub async fn search_knowledge_entries(pool: &PgPool)",
                "db/knowledge.rs",
                &["search_knowledge".into()],
            ),
            IdentifierRole::Mention
        );
        assert_eq!(
            identifier_role(
                "pub enum ResyncReason {\n    RemoteChanged,\n    MissingEmbeddings,\n}",
                "workers/source_resync.rs",
                &["MissingEmbeddings".into()],
            ),
            IdentifierRole::Defined
        );
        assert_eq!(
            identifier_role(
                "symbol: tests.quoted_tool\nfn name(&self) -> &str { \"search_knowledge\" }",
                "embeddings/ranker.rs",
                &["search_knowledge".into()],
            ),
            IdentifierRole::Mention
        );
        assert_eq!(
            identifier_role(
                "pub use github::GitHubAdapter;",
                "adapters/mod.rs",
                &["GitHubAdapter".into()],
            ),
            IdentifierRole::Defined
        );
        assert_eq!(
            identifier_role(
                "pub live_uris: Vec<String>,",
                "content/mod.rs",
                &["live_uris".into()],
            ),
            IdentifierRole::Defined
        );
        assert_eq!(
            identifier_role(
                r#"let query = "What does should_skip_blob do?";
            "pub fn should_skip_blob(&self, uri: &str, blob_sha: &str) -> bool { true }",
"#,
                "embeddings/local_rerank.rs",
                &["should_skip_blob".into()],
            ),
            IdentifierRole::Mention
        );
        assert_eq!(
            identifier_role(
                "let method = score_hit(query, \"ollama.rs\", \"format!(/api/embeddings)\");",
                "embeddings/ranker.rs",
                &["/api/embeddings".into()],
            ),
            IdentifierRole::Mention
        );
    }

    #[test]
    fn tool_name_window_ending_inside_a_multibyte_character_does_not_panic() {
        let text = format!("fn name(){}", "é".repeat(100));
        assert!(!text.is_char_boundary(TOOL_NAME_WINDOW));
        assert!(!tool_name_definition(&text, "search_knowledge"));
        assert_eq!(
            identifier_role(&text, "agent/tools.rs", &["search_knowledge".into()]),
            IdentifierRole::None
        );
    }

    #[test]
    fn tool_name_window_still_finds_a_quoted_name_before_multibyte_text() {
        let text = format!(
            "fn name(&self) -> &str {{ \"search_knowledge\" }}{}",
            "é".repeat(100)
        );
        assert!(!text.is_char_boundary(TOOL_NAME_WINDOW));
        assert!(tool_name_definition(&text, "search_knowledge"));
        assert_eq!(
            identifier_role(&text, "agent/tools.rs", &["search_knowledge".into()]),
            IdentifierRole::Defined
        );
    }

    #[test]
    fn role_strength_orders_the_roles() {
        let identifiers = ["should_skip_blob".to_string()];
        assert_eq!(
            role_strength("pub fn should_skip_blob() {}", "a.rs", &identifiers),
            3
        );
        assert_eq!(
            role_strength("fn should_skip_blob() {}", "a.rs", &identifiers),
            2
        );
        assert_eq!(
            role_strength("calls should_skip_blob", "a.rs", &identifiers),
            1
        );
        assert_eq!(role_strength("nothing", "a.rs", &identifiers), 0);
    }

    #[test]
    fn identifier_counts_and_part_coverage() {
        let identifiers = ["should_skip_blob".to_string(), "live_uris".to_string()];
        assert_eq!(
            identifier_hit_count("should_skip_blob(live_uris)", &identifiers),
            2
        );
        assert_eq!(identifier_hit_count("SKIP", &identifiers), 0);
        assert_eq!(
            identifier_part_coverage("SKIP BLOB uris", &identifiers),
            0.6
        );
        assert_eq!(identifier_part_coverage("anything", &[]), 0.0);
    }
}
