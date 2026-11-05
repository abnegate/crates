use crate::pull_request::graphql_auto_merge_request::GraphQlAutoMergeRequest;
use serde::Deserialize;

/// Whether a pull request is a draft and set to merge once it is ready, and
/// what the token may change about that, as GitHub's GraphQL API answers for
/// it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlAutomation {
    pub(super) id: String,
    pub(super) is_draft: bool,
    #[serde(default)]
    pub(super) auto_merge_request: Option<GraphQlAutoMergeRequest>,
    pub(super) viewer_can_enable_auto_merge: bool,
    pub(super) viewer_can_disable_auto_merge: bool,
}
