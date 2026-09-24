use crate::pull_request::graphql_connection::GraphQlConnection;
use crate::pull_request::graphql_thread_comment::GraphQlThreadComment;
use serde::Deserialize;

/// One review thread, as GitHub's GraphQL API answers for it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlReviewThread {
    pub(super) id: String,
    pub(super) is_resolved: bool,
    pub(super) is_outdated: bool,
    #[serde(default)]
    pub(super) path: Option<String>,
    #[serde(default)]
    pub(super) line: Option<u32>,
    pub(super) comments: GraphQlConnection<GraphQlThreadComment>,
}
