use serde::Deserialize;

/// Whoever wrote something, as GitHub's GraphQL API names them.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GraphQlActor {
    #[serde(default)]
    pub(super) login: Option<String>,
}
