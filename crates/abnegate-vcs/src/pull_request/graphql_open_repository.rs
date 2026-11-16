use crate::pull_request::graphql_connection::GraphQlConnection;
use crate::pull_request::graphql_listed_pull_request::GraphQlListedPullRequest;
use serde::Deserialize;

/// A repository in a GraphQL answer, known by its node id, with one page of
/// its open pull requests.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlOpenRepository {
    pub(super) id: String,
    pub(super) pull_requests: GraphQlConnection<GraphQlListedPullRequest>,
}
