use crate::pull_request::github_commit_status::GitHubCommitStatus;
use serde::Deserialize;

/// One page of a commit's combined status, each status read as `R`, and how
/// many statuses there are in all when GitHub says.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubCombinedStatus<R = GitHubCommitStatus> {
    #[serde(default)]
    pub(super) total_count: Option<u64>,
    pub(super) statuses: Vec<R>,
}
