use crate::pull_request::github_repository_name::GitHubRepositoryName;
use serde::Deserialize;

/// One workflow run as GitHub's REST API lists it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubWorkflowRun {
    #[serde(default)]
    pub(super) id: u64,
    #[serde(default)]
    pub(super) workflow_id: Option<u64>,
    #[serde(default)]
    pub(super) run_attempt: Option<u64>,
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) path: String,
    #[serde(default)]
    pub(super) event: String,
    #[serde(default)]
    pub(super) status: Option<String>,
    #[serde(default)]
    pub(super) conclusion: Option<String>,
    #[serde(default)]
    pub(super) head_branch: Option<String>,
    #[serde(default)]
    pub(super) head_sha: Option<String>,
    #[serde(default)]
    pub(super) created_at: String,
    #[serde(default)]
    pub(super) run_started_at: Option<String>,
    #[serde(default)]
    pub(super) updated_at: String,
    #[serde(default)]
    pub(super) html_url: String,
    #[serde(default)]
    pub(super) repository: Option<GitHubRepositoryName>,
}
