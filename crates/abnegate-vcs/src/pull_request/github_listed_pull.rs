use crate::pull_request::PullRequestState;
use crate::pull_request::github_listed_branch::GitHubListedBranch;
use serde::Deserialize;
use std::num::NonZeroU64;

/// A pull request in a list GitHub's REST API returns.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubListedPull {
    pub(super) node_id: String,
    pub(super) number: NonZeroU64,
    pub(super) html_url: String,
    pub(super) state: PullRequestState,
    pub(super) head: GitHubListedBranch,
    pub(super) base: GitHubListedBranch,
    pub(super) title: String,
    #[serde(default)]
    pub(super) draft: Option<bool>,
    #[serde(default)]
    pub(super) merged_at: Option<String>,
    #[serde(default)]
    pub(super) updated_at: Option<String>,
}
