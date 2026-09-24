use crate::pull_request::GitHubBranch;
use crate::pull_request::PullRequestState;
use serde::Deserialize;
use serde::Serialize;
use std::num::NonZeroU64;

/// A pull request as the GitHub REST API returns it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct GitHubPullRequest {
    /// GitHub's own identifier.
    pub id: u64,
    /// The number it goes by in its repository.
    pub number: NonZeroU64,
    /// Where it can be read.
    pub html_url: String,
    /// Whether it is open.
    pub state: PullRequestState,
    /// Its title.
    pub title: String,
    /// Its body, if it has one.
    pub body: Option<String>,
    /// The branch it merges from.
    pub head: GitHubBranch,
    /// The branch it merges into.
    pub base: GitHubBranch,
}
