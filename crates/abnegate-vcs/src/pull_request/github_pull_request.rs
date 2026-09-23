use crate::pull_request::GitHubBranch;
use crate::pull_request::PullRequestState;
use serde::Deserialize;
use serde::Serialize;
use std::num::NonZeroU64;

/// A pull request as the GitHub REST API returns it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubPullRequest {
    pub id: u64,
    pub number: NonZeroU64,
    pub html_url: String,
    pub state: PullRequestState,
    pub title: String,
    pub body: Option<String>,
    pub head: GitHubBranch,
    pub base: GitHubBranch,
}
