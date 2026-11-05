use serde::Deserialize;
use std::num::NonZeroU64;

/// One job of a workflow run as GitHub's REST API returns it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubWorkflowJob {
    pub(super) id: NonZeroU64,
    pub(super) run_id: NonZeroU64,
    pub(super) name: String,
    pub(super) status: String,
    #[serde(default)]
    pub(super) conclusion: Option<String>,
    pub(super) head_sha: String,
    #[serde(default)]
    pub(super) html_url: Option<String>,
    #[serde(default)]
    pub(super) started_at: Option<String>,
    #[serde(default)]
    pub(super) completed_at: Option<String>,
}
