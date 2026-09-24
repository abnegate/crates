use crate::pull_request::graphql_connection::GraphQlConnection;
use crate::pull_request::graphql_review_thread::GraphQlReviewThread;
use serde::Deserialize;

/// A pull request in a GraphQL answer, holding one page of its review
/// threads.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlPullRequest {
    pub(super) review_threads: GraphQlConnection<GraphQlReviewThread>,
}
