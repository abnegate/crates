use crate::pull_request::graphql_actor::GraphQlActor;
use serde::Deserialize;

/// The auto-merge set on a pull request, as GitHub's GraphQL API answers for
/// it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlAutoMergeRequest {
    #[serde(default)]
    pub(super) merge_method: Option<String>,
    #[serde(default)]
    pub(super) enabled_at: Option<String>,
    #[serde(default)]
    pub(super) enabled_by: Option<GraphQlActor>,
    #[serde(default)]
    pub(super) commit_headline: Option<String>,
    #[serde(default)]
    pub(super) commit_body: Option<String>,
}
