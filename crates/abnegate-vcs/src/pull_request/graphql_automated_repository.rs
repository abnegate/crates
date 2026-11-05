use crate::pull_request::graphql_automation::GraphQlAutomation;
use crate::pull_request::graphql_merge_settings::GraphQlMergeSettings;
use serde::Deserialize;

/// A repository in a GraphQL answer, with how it lets its pull requests merge
/// and the pull request asked for, which GitHub leaves null when the
/// repository has no such pull request.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlAutomatedRepository {
    #[serde(flatten)]
    pub(super) settings: GraphQlMergeSettings,
    #[serde(default)]
    pub(super) pull_request: Option<GraphQlAutomation>,
}
