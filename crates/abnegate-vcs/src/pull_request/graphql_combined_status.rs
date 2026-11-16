use crate::pull_request::graphql_status_context::GraphQlStatusContext;
use serde::Deserialize;

/// The latest status every context reported on a commit, as GitHub's GraphQL
/// API answers for it: a list, never a page of one.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GraphQlCombinedStatus {
    #[serde(default)]
    pub(super) contexts: Vec<GraphQlStatusContext>,
}
