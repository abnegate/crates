use crate::pull_request::PullRequestState;
use std::num::NonZeroU64;

/// A pull request that was just opened.
#[derive(Debug, Clone)]
pub struct CreatedPullRequest {
    /// Where the pull request can be read.
    pub url: String,
    /// The number GitHub gave it.
    pub number: NonZeroU64,
    /// Whether it is open.
    pub state: PullRequestState,
}
