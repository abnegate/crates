use crate::pull_request::github_check_run::GitHubCheckRun;
use serde::Deserialize;

/// One page of the check runs on a commit, and how many there are in all
/// when GitHub says.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubCheckRuns {
    #[serde(default)]
    pub(super) total_count: Option<u64>,
    pub(super) check_runs: Vec<GitHubCheckRun>,
}
