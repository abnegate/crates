use crate::pull_request::PullRequestState;
use std::num::NonZeroU64;

/// A pull request that was just opened.
#[derive(Debug, Clone)]
pub struct CreatedPullRequest {
    pub url: String,
    pub number: NonZeroU64,
    pub state: PullRequestState,
}
