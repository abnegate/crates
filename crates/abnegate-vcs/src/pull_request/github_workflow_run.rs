use crate::pull_request::github_repository_name::GitHubRepositoryName;
use serde::Deserialize;
use std::num::NonZeroU64;

/// One workflow run as GitHub's REST API lists it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubWorkflowRun {
    pub(super) id: NonZeroU64,
    pub(super) workflow_id: NonZeroU64,
    #[serde(default)]
    pub(super) run_attempt: Option<u32>,
    #[serde(default)]
    pub(super) name: Option<String>,
    pub(super) path: String,
    pub(super) event: String,
    #[serde(default)]
    pub(super) status: Option<String>,
    #[serde(default)]
    pub(super) conclusion: Option<String>,
    #[serde(default)]
    pub(super) head_branch: Option<String>,
    pub(super) head_sha: String,
    pub(super) created_at: String,
    #[serde(default)]
    pub(super) run_started_at: Option<String>,
    pub(super) updated_at: String,
    pub(super) html_url: String,
    pub(super) repository: GitHubRepositoryName,
}
