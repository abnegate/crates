use serde::Deserialize;

/// The latest status one context reported on a commit, as GitHub's GraphQL
/// API answers for it, its state in GitHub's capitals.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlStatusContext {
    pub(super) context: String,
    pub(super) state: String,
    #[serde(default)]
    pub(super) description: Option<String>,
    #[serde(default)]
    pub(super) target_url: Option<String>,
    #[serde(default)]
    pub(super) created_at: Option<String>,
}
