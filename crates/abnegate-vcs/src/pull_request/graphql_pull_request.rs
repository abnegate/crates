use crate::pull_request::graphql_connection::GraphQlConnection;
use serde::Deserialize;

/// A pull request in a GraphQL answer, holding one page of its review
/// threads, each read as `T`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlPullRequest<T> {
    pub(super) review_threads: GraphQlConnection<T>,
}
