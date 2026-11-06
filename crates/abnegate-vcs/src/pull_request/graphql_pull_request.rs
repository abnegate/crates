use crate::pull_request::graphql_connection::GraphQlConnection;
use serde::Deserialize;

/// A pull request in a GraphQL answer, the number GitHub answered for, and
/// one page of its review threads, each read as `T`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlPullRequest<T> {
    #[serde(default)]
    pub(super) number: Option<u64>,
    pub(super) review_threads: GraphQlConnection<T>,
}
