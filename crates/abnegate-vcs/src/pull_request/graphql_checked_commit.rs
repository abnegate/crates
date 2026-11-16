use crate::pull_request::graphql_check_suite::GraphQlCheckSuite;
use crate::pull_request::graphql_combined_status::GraphQlCombinedStatus;
use crate::pull_request::graphql_connection::GraphQlConnection;
use serde::Deserialize;

/// A commit with its combined status and a page of its check suites, as
/// GitHub's GraphQL API answers for it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlCheckedCommit {
    pub(super) oid: String,
    #[serde(default)]
    pub(super) status: Option<GraphQlCombinedStatus>,
    pub(super) check_suites: GraphQlConnection<GraphQlCheckSuite>,
}
