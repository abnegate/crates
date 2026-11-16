use crate::pull_request::graphql_actor::GraphQlActor;
use serde::Deserialize;

/// A pull request as a page of a repository's pull requests lists it, as
/// GitHub's GraphQL API answers for it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlListedPullRequest {
    pub(super) id: String,
    pub(super) number: u64,
    pub(super) title: String,
    pub(super) url: String,
    pub(super) state: String,
    pub(super) is_draft: bool,
    #[serde(default)]
    pub(super) updated_at: Option<String>,
    pub(super) head_ref_name: String,
    #[serde(default)]
    pub(super) head_ref_oid: Option<String>,
    pub(super) base_ref_name: String,
    #[serde(default)]
    pub(super) author: Option<GraphQlActor>,
}
