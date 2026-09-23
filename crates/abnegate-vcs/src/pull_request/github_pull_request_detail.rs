use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubPullRequestDetail {
    #[serde(default)]
    pub(super) created_at: Option<String>,
    #[serde(default)]
    pub(super) merged_at: Option<String>,
    #[serde(default)]
    pub(super) state: Option<String>,
    #[serde(default)]
    pub(super) mergeable: Option<bool>,
}
