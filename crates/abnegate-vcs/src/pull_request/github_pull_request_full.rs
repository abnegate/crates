use crate::pull_request::GitHubBranch;
use crate::pull_request::MergeableState;
use crate::pull_request::PullRequestState;
use serde::Deserialize;
use std::num::NonZeroU64;

/// A pull request as the GitHub REST API returns it when it is read on its
/// own. What identifies it is required; what GitHub computes in the
/// background, or leaves out of a smaller answer, falls back to its default.
#[derive(Debug, Deserialize)]
pub(super) struct GitHubPullRequestFull {
    pub(super) node_id: String,
    pub(super) number: NonZeroU64,
    pub(super) title: String,
    pub(super) state: PullRequestState,
    pub(super) head: GitHubBranch,
    pub(super) base: GitHubBranch,
    pub(super) html_url: String,
    #[serde(default)]
    pub(super) body: Option<String>,
    #[serde(default)]
    pub(super) draft: bool,
    #[serde(default)]
    pub(super) merged: bool,
    #[serde(default)]
    pub(super) merge_commit_sha: Option<String>,
    #[serde(default)]
    pub(super) mergeable: Option<bool>,
    #[serde(default)]
    pub(super) mergeable_state: Option<MergeableState>,
    #[serde(default)]
    pub(super) changed_files: u32,
    #[serde(default)]
    pub(super) additions: u32,
    #[serde(default)]
    pub(super) deletions: u32,
    #[serde(default)]
    pub(super) commits: u32,
}
