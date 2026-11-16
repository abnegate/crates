use serde::Deserialize;

/// Anything GitHub's GraphQL API answers for, known only by its node id.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GraphQlNode {
    pub(super) id: String,
}
