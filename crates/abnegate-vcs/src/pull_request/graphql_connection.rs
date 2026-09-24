use crate::pull_request::graphql_page_info::GraphQlPageInfo;
use serde::Deserialize;

/// One page of a GraphQL connection: the nodes on it, and where it ended when
/// the query asked.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlConnection<T> {
    pub(super) nodes: Vec<T>,
    #[serde(default)]
    pub(super) page_info: Option<GraphQlPageInfo>,
}
