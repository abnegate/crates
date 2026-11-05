use crate::pull_request::github_workflow_run::GitHubWorkflowRun;
use serde::Deserialize;

/// One page of a repository's workflow runs, and how many match in all when
/// GitHub says.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubWorkflowRuns {
    #[serde(default)]
    pub(super) total_count: Option<u64>,
    pub(super) workflow_runs: Vec<GitHubWorkflowRun>,
}
