use serde::Deserialize;

/// A repository in a GraphQL answer, holding the pull request asked for,
/// which GitHub leaves null when the repository has no such pull request.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlRepository<T> {
    pub(super) pull_request: Option<T>,
}
