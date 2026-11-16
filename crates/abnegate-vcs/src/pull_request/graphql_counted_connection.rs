use serde::Deserialize;

/// One page of a GraphQL connection and how many nodes GitHub counts in the
/// whole of it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlCountedConnection<T> {
    pub(super) total_count: u64,
    pub(super) nodes: Vec<T>,
}
