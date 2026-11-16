use crate::pull_request::graphql_actor::GraphQlActor;
use crate::pull_request::graphql_big_int::GraphQlBigInt;
use serde::Deserialize;

/// A comment on a pull request's conversation, as GitHub's GraphQL API
/// answers for it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlIssueComment {
    pub(super) full_database_id: GraphQlBigInt,
    #[serde(default)]
    pub(super) body: Option<String>,
    pub(super) url: String,
    pub(super) created_at: String,
    #[serde(default)]
    pub(super) author: Option<GraphQlActor>,
}
