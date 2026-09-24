use serde::Deserialize;

/// One error GitHub's GraphQL API reported, and the kind it named, if any.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GraphQlError {
    #[serde(default)]
    pub(super) message: String,
    #[serde(default, rename = "type")]
    pub(super) kind: Option<String>,
}
