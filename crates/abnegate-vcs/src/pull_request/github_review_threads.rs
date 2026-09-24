use crate::pull_request::github_review_thread::GitHubReviewThread;
use crate::pull_request::graphql_connection::GraphQlConnection;
use serde::Deserialize;

/// One page of a pull request's review threads, as GitHub's GraphQL API
/// answers for it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GitHubReviewThreads {
    pub(super) review_threads: GraphQlConnection<GitHubReviewThread>,
}
