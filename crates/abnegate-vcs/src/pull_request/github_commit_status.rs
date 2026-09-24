use crate::pull_request::github_status_state::GitHubStatusState;
use serde::Deserialize;

/// The latest status one context reported on a commit.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubCommitStatus {
    pub(super) context: String,
    pub(super) state: GitHubStatusState,
}
