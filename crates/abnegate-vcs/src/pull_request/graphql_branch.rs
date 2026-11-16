use serde::Deserialize;

/// A branch as GitHub's GraphQL API names it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GraphQlBranch {
    pub(super) name: String,
}
