use serde::Deserialize;

/// One job of a workflow run as GitHub's REST API returns it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubWorkflowJob {
    pub(super) id: u64,
    #[serde(default)]
    pub(super) run_id: Option<u64>,
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) status: String,
    #[serde(default)]
    pub(super) conclusion: Option<String>,
    #[serde(default)]
    pub(super) head_sha: Option<String>,
    #[serde(default)]
    pub(super) html_url: Option<String>,
    #[serde(default)]
    pub(super) started_at: Option<String>,
    #[serde(default)]
    pub(super) completed_at: Option<String>,
}
