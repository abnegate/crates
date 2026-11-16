use serde::Deserialize;

/// The GitHub App a check suite belongs to, as GitHub's GraphQL API names it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GraphQlApp {
    #[serde(default)]
    pub(super) slug: Option<String>,
}
