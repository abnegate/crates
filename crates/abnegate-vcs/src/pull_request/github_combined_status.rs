use crate::pull_request::github_commit_status::GitHubCommitStatus;
use serde::Deserialize;

/// One page of a commit's combined status, and how many statuses there are
/// in all when GitHub says.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubCombinedStatus {
    #[serde(default)]
    pub(super) total_count: Option<u64>,
    pub(super) statuses: Vec<GitHubCommitStatus>,
}
