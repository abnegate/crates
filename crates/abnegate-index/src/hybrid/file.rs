//! Keeping the best chunk of each file, then a few extra chunks per file.

use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::Hash;

use crate::hybrid::compare::compare_file_chunks;
use crate::hybrid::question::Question;
use crate::hybrid::result::HybridSearchResult;
use crate::rank::is_fixture_uri;
use crate::rank::is_module_prelude;
use crate::rank::is_outline_chunk;
use crate::rank::is_test_chunk;

/// Keep one row per file first, so more distinct files fit in `limit`, then
/// fill the slots left with up to `maximum_per_file` chunks per file in
/// `results` order.
///
/// A file is an item URI without its `@ref`. Its row is the chunk that best
/// answers `query`: body over outline or prelude over test or fixture, then a
/// definition that answers the question, coverage of the question's words,
/// identifiers and symbols, role strength, the lexical cross-encoder, and
/// finally the score. Rows are ordered body first, then files whose path the
/// question names, otherwise in first-seen file order. A `limit` or
/// `maximum_per_file` of 0 keeps nothing.
pub fn cap_per_file<Id: Clone + Eq + Hash>(
    results: Vec<HybridSearchResult<Id>>,
    maximum_per_file: usize,
    limit: usize,
    query: &str,
) -> Vec<HybridSearchResult<Id>> {
    if limit == 0 || maximum_per_file == 0 {
        return Vec::new();
    }
    let question = Question::new(query);
    let keys: Vec<String> = results
        .iter()
        .map(|result| file_key(&result.item_uri).to_string())
        .collect();
    let mut group_positions: HashMap<&str, usize> = HashMap::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        let position = *group_positions.entry(key.as_str()).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[position].push(index);
    }
    let best: Vec<usize> = groups
        .iter()
        .filter_map(|indices| {
            indices.iter().copied().max_by(|&left, &right| {
                compare_file_chunks(&question, &results[left], &results[right])
            })
        })
        .collect();

    let mut slots: Vec<Option<HybridSearchResult<Id>>> = results.into_iter().map(Some).collect();
    let mut taken: HashSet<Id> = HashSet::new();
    let mut chosen: Vec<HybridSearchResult<Id>> = Vec::with_capacity(limit.min(slots.len()));
    for index in best {
        if let Some(result) = slots[index].take() {
            taken.insert(result.id.clone());
            chosen.push(result);
        }
    }
    chosen.sort_by(|left, right| {
        file_row_tier(&right.item_uri, &right.chunk_text)
            .cmp(&file_row_tier(&left.item_uri, &left.chunk_text))
            .then_with(|| {
                uri_path_hit(&question, &right.item_uri)
                    .cmp(&uri_path_hit(&question, &left.item_uri))
            })
    });
    chosen.truncate(limit);
    if maximum_per_file > 1 && chosen.len() < limit {
        let mut extras: HashMap<&str, usize> = HashMap::new();
        for (slot, key) in slots.iter_mut().zip(&keys) {
            if chosen.len() >= limit {
                break;
            }
            let Some(result) = slot.as_ref() else {
                continue;
            };
            if taken.contains(&result.id) {
                continue;
            }
            let count = extras.entry(key.as_str()).or_insert(0);
            if *count + 1 < maximum_per_file {
                *count += 1;
                taken.insert(result.id.clone());
                chosen.extend(slot.take());
            }
        }
    }
    chosen
}

pub(super) fn file_row_tier(uri: &str, text: &str) -> u8 {
    if is_test_chunk(uri, text) || is_fixture_uri(uri) {
        return 0;
    }
    if is_outline_chunk(text) || is_module_prelude(text) {
        return 1;
    }
    2
}

fn file_key(uri: &str) -> &str {
    uri.split('@')
        .next()
        .filter(|stem| !stem.is_empty())
        .unwrap_or(uri)
}

fn uri_path_hit(question: &Question<'_>, uri: &str) -> u8 {
    let lowercase_uri = uri.to_ascii_lowercase();
    u8::from(question.path_tokens.iter().any(|token| {
        lowercase_uri.contains(&format!("/{token}."))
            || lowercase_uri.contains(&format!("/{token}@"))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hybrid::sample::hit;

    fn kept_first(results: Vec<HybridSearchResult<u128>>, query: &str) -> String {
        cap_per_file(results, 2, 2, query)
            .into_iter()
            .next()
            .map(|result| result.chunk_text)
            .unwrap_or_default()
    }

    #[test]
    fn cap_per_file_prefers_filename_type_over_same_symbol_guard() {
        let authorization = hit(
            1,
            "github://acme/widgets/routes/artifacts.rs@main",
            "symbol: get\nkind: Function\nlet authorized = match chats::get_chat(state.db(), chat_id).await {\n    // Do not reveal whether an artifact exists\n};",
            2.8,
        );
        let store = hit(
            2,
            "github://acme/widgets/routes/artifacts.rs@main",
            "symbol: get\nkind: Function\nlet store = ArtifactStore::new(state.config().comfyui.artifact_root.clone());\nmatch store.read(workspace_id, chat_id, owner_id, &filename).await {",
            0.4,
        );
        let kept = kept_first(
            vec![authorization, store],
            "How are generated chat artifacts authorized before serving?",
        );
        assert!(kept.contains("ArtifactStore"), "kept {kept}");
    }

    #[test]
    fn cap_per_file_surfaces_path_named_file() {
        let noise = (0..10u128).map(|index| {
            hit(
                index + 1,
                &format!("github://acme/widgets/other/file{index}.rs@main"),
                "fn helper() {}",
                2.0,
            )
        });
        let artifact = hit(
            20,
            "github://acme/widgets/routes/artifacts.rs@main",
            "//! Authorized generated-artifact serving.\nasync fn get(State(state): State<AppState>)",
            0.1,
        );
        let capped = cap_per_file(
            noise.chain(std::iter::once(artifact)).collect(),
            2,
            10,
            "How are generated chat artifacts authorized before serving?",
        );
        let uris: Vec<&str> = capped.iter().map(|row| row.item_uri.as_str()).collect();
        assert!(
            uris.iter().any(|uri| uri.contains("artifacts.rs")),
            "uris {uris:?}"
        );
        assert!(
            uris.iter().take(2).any(|uri| uri.contains("artifacts.rs")),
            "top {:?}",
            uris.iter().take(2).collect::<Vec<_>>()
        );
    }

    #[test]
    fn cap_per_file_prefers_unique_files() {
        let results = (0..3u128)
            .flat_map(|file| {
                (0..2u128).map(move |chunk| {
                    HybridSearchResult::new(
                        file * 2 + chunk + 1,
                        format!("chunk {file}-{chunk}"),
                        format!("file{file}.rs"),
                        format!("file{file}.rs"),
                        1.0 - file as f32 * 0.1 - chunk as f32 * 0.01,
                    )
                    .with_fusion_score(1.0)
                })
            })
            .collect();
        let capped = cap_per_file(results, 2, 4, "unique files");
        let unique: HashSet<&str> = capped.iter().map(|row| row.item_uri.as_str()).collect();
        assert_eq!(unique.len(), 3);
        assert_eq!(capped.len(), 4);
    }

    #[test]
    fn cap_per_file_uses_best_chunk_as_file_row() {
        let header = HybridSearchResult::new(
            1u128,
            "path: github://acme/widgets/services/email.rs kind: file_header\n\nidentifiers: send_email",
            "github://acme/widgets/services/email.rs@main",
            "email.rs",
            3.0,
        )
        .with_keyword_rank(1)
        .with_keyword_score(0.4);
        let body = HybridSearchResult::new(
            2u128,
            r#"return Err("Email service not configured");"#,
            "github://acme/widgets/services/email.rs@main",
            "email.rs",
            1.0,
        )
        .with_keyword_rank(2)
        .with_keyword_score(0.2);
        let kept = kept_first(
            vec![header, body],
            "What error is returned when SMTP is not configured?",
        );
        assert!(kept.contains("Email service not configured"), "kept {kept}");
    }

    #[test]
    fn cap_per_file_prefers_identifier_mention_over_same_file_neighbor() {
        let neighbor = HybridSearchResult::new(
            1u128,
            "pub async fn list_chats(state: AppState) -> Result<Vec<Chat>>",
            "github://acme/widgets/routes/chats.rs@main",
            "chats.rs",
            4.0,
        )
        .with_keyword_rank(1)
        .with_keyword_score(0.5);
        let mention = HybridSearchResult::new(
            2u128,
            "get(search_messages) // GET /api/chats/search",
            "github://acme/widgets/routes/chats.rs@main",
            "chats.rs",
            1.1,
        )
        .with_keyword_rank(2)
        .with_keyword_score(0.2);
        let kept = kept_first(
            vec![neighbor, mention],
            "Where is search_messages used by GET /api/chats/search?",
        );
        assert!(kept.contains("search_messages"), "kept {kept}");
    }

    #[test]
    fn cap_per_file_prefers_error_phrase_over_from_env() {
        let from_environment = hit(
            1,
            "github://acme/widgets/services/email.rs@main",
            r#"symbol: EmailConfig.from_env
pub fn from_env() -> EmailResult<Self> {
    env::var("SMTP_HOST").map_err(|_| EmailError::NotConfigured)
}"#,
            3.0,
        );
        let error = hit(
            2,
            "github://acme/widgets/services/email.rs@main",
            r#"symbol: EmailError
#[error("Email service not configured")]
NotConfigured,"#,
            1.0,
        );
        let kept = kept_first(
            vec![from_environment, error],
            "What error is returned when SMTP is not configured?",
        );
        assert!(kept.contains("Email service not configured"), "kept {kept}");
    }

    #[test]
    fn cap_per_file_prefers_helper_over_tool_name() {
        let name = hit(
            1,
            "github://acme/widgets/tools/command.rs@main",
            "symbol: RunCommandTool.name\nfn name(&self) -> &str {\n        \"run_command\"\n    }",
            3.5,
        );
        let helper = hit(
            2,
            "github://acme/widgets/tools/command.rs@main",
            "symbol: trim_middle\nfn trim_middle(text: &str) -> String { chars }",
            0.4,
        );
        let kept = kept_first(
            vec![name, helper],
            "How does run_command trim oversized stdout?",
        );
        assert!(kept.contains("trim_middle"), "kept {kept}");
    }

    #[test]
    fn cap_per_file_prefers_trim_over_execute_stdout() {
        let execute = hit(
            1,
            "github://acme/widgets/core/src/tools/command.rs@main",
            r#"symbol: RunCommandTool.execute
kind: Method
Parent: RunCommandTool
Signature: async fn execute(&self, params: Value, context: &ToolContext) -> Result<ToolResult, ToolError>

        let mut cmd = Command::new(&params.command);
        cmd.args(&params.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
"#,
            4.0,
        );
        let helper = hit(
            2,
            "github://acme/widgets/core/src/tools/command.rs@main",
            r#"symbol: trim_middle
kind: Function
Signature: fn trim_middle(text: &str) -> String

fn trim_middle(text: &str) -> String {
    format!("{head}\n\n[… {} characters trimmed …]\n\n{tail}", chars.len())
}
"#,
            0.2,
        );
        let kept = kept_first(
            vec![execute, helper],
            "How does run_command trim oversized stdout?",
        );
        assert!(kept.contains("trim_middle"), "kept {kept}");
    }

    #[test]
    fn cap_per_file_prefers_cleanup_over_constructor() {
        let constructor = hit(
            1,
            "github://acme/widgets/utils/rate_limit.rs@main",
            "symbol: RateLimiter.new\nkind: Method\n/// Create a new rate limiter with the given configuration\npub fn new(config: RateLimitConfig)",
            3.4,
        );
        let cleanup = hit(
            2,
            "github://acme/widgets/utils/rate_limit.rs@main",
            "symbol: RateLimiter.cleanup\nkind: Method\n/// Clean up old entries (optional, for memory management)\npub fn cleanup(&self)",
            0.4,
        );
        let kept = kept_first(
            vec![constructor, cleanup],
            "How does the rate limiter prevent unbounded memory growth?",
        );
        assert!(kept.contains("fn cleanup"), "kept {kept}");
    }

    #[test]
    fn cap_per_file_prefers_cleanup_over_check() {
        let check = hit(
            1,
            "github://acme/widgets/utils/rate_limit.rs@main",
            "symbol: RateLimiter.check_rate_limit\nkind: Method\nParent: RateLimiter\n// This prevents race condition where two concurrent requests both see count=9\npub fn check_rate_limit(&self, user_id: Uuid)",
            3.0,
        );
        let cleanup = hit(
            2,
            "github://acme/widgets/utils/rate_limit.rs@main",
            "symbol: RateLimiter.cleanup\nParent: RateLimiter\n/// Clean up old entries (optional, for memory management)\npub fn cleanup(&self)",
            0.5,
        );
        let kept = kept_first(
            vec![check, cleanup],
            "How does the rate limiter prevent unbounded memory growth?",
        );
        assert!(kept.contains("fn cleanup"), "kept {kept}");
    }

    #[test]
    fn cap_per_file_prefers_identifier_method_over_type_banner() {
        let claims = hit(
            1,
            "github://acme/widgets/auth/jwt.rs@main",
            "symbol: Claims\nkind: Struct\n/// Subject (user ID)\npub struct Claims { pub sub: String }",
            3.2,
        );
        let user_id = hit(
            2,
            "github://acme/widgets/auth/jwt.rs@main",
            "symbol: Claims.user_id\nkind: Method\npub fn user_id(&self) -> Result<Uuid, uuid::Error> {\n        Uuid::parse_str(&self.sub)\n    }",
            0.7,
        );
        let kept = kept_first(
            vec![claims, user_id],
            "How does user_id parse the JWT subject on Claims?",
        );
        assert!(kept.contains("parse_str"), "kept {kept}");
    }

    #[test]
    fn cap_per_file_prefers_body_over_module_prelude() {
        let prelude = hit(
            1,
            "github://acme/widgets/routes/artifacts.rs@main",
            "Language: rust\nkind: top_level\n\n//! Authorized generated-artifact serving.\n",
            2.8,
        );
        let body = hit(
            2,
            "github://acme/widgets/routes/artifacts.rs@main",
            "symbol: get_artifact\nlet authorized = chats::get_chat\nlet store = ArtifactStore::new",
            0.6,
        );
        let kept = kept_first(
            vec![prelude, body],
            "How are generated chat artifacts authorized before serving?",
        );
        assert!(kept.contains("ArtifactStore"), "kept {kept}");
    }

    #[test]
    fn cap_per_file_prefers_question_coverage_over_bare_definition() {
        let definition = HybridSearchResult::new(
            1u128,
            "pub async fn run_command(cmd: &str) -> Result<Output> { spawn(cmd) }",
            "github://acme/widgets/tools/command.rs@main",
            "command.rs",
            3.5,
        )
        .with_keyword_rank(1)
        .with_keyword_score(0.5);
        let body = HybridSearchResult::new(
            2u128,
            "fn trim_middle(stdout: &str) { /* oversized stdout */ }",
            "github://acme/widgets/tools/command.rs@main",
            "command.rs",
            1.0,
        )
        .with_keyword_rank(2)
        .with_keyword_score(0.2);
        let kept = kept_first(
            vec![definition, body],
            "How does run_command trim oversized stdout?",
        );
        assert!(kept.contains("trim_middle"), "kept {kept}");
    }

    #[test]
    fn cap_per_file_prefers_identifier_conjunction_over_gather_prefix() {
        let gather = hit(
            1,
            "github://acme/widgets/context/service.rs@main",
            "symbol: ContextService.gather\nkind: Method\npub async fn gather(&self) {\n    result.items_unchanged += fetch_result.stats.items_skipped;\n}",
            3.5,
        );
        let call = hit(
            2,
            "github://acme/widgets/context/service.rs@main",
            "symbol: ContextService.gather\nkind: Method\nlet uris = fetch_result.live_uris.clone();\nstore.retain_content_uris(source.id, &uris)\n}",
            0.3,
        );
        let kept = kept_first(
            vec![gather, call],
            "Why must retain_content_uris use live_uris after an incremental gather?",
        );
        assert!(
            kept.contains("retain_content_uris") && kept.contains("live_uris"),
            "kept {kept}"
        );
    }

    #[test]
    fn a_zero_limit_or_per_file_cap_keeps_nothing() {
        let row = hit(1, "src/a.rs", "fn a() {}", 1.0);
        assert!(cap_per_file(vec![row.clone()], 2, 0, "a").is_empty());
        assert!(cap_per_file(vec![row], 0, 2, "a").is_empty());
    }

    #[test]
    fn a_file_key_drops_the_ref() {
        assert_eq!(
            file_key("github://acme/widgets/a.rs@main"),
            "github://acme/widgets/a.rs"
        );
        assert_eq!(file_key("@main"), "@main");
    }
}
