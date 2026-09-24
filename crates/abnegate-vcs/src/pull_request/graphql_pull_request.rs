use serde::Deserialize;

/// The pull request a GraphQL query asked a repository for, which GitHub
/// leaves null when the repository has no such pull request.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlPullRequest<T> {
    pub(super) pull_request: Option<T>,
}
