use serde::Deserialize;

/// How a repository lets its pull requests merge, as GitHub's GraphQL API
/// answers for it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlMergeSettings {
    pub(super) auto_merge_allowed: bool,
    pub(super) squash_merge_allowed: bool,
    pub(super) merge_commit_allowed: bool,
    pub(super) rebase_merge_allowed: bool,
}
