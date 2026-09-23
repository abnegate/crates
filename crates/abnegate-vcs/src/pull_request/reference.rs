use crate::pull_request::Repository;
use std::num::NonZeroU64;

/// Where a pull request lives, recovered from the URL a run recorded. Only
/// [`crate::pull_request::PullRequestService::pull_request`] makes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestReference {
    repository: Repository,
    number: NonZeroU64,
}

impl PullRequestReference {
    pub(super) fn new(repository: Repository, number: NonZeroU64) -> Self {
        Self { repository, number }
    }

    pub fn repository(&self) -> &Repository {
        &self.repository
    }

    pub fn number(&self) -> NonZeroU64 {
        self.number
    }
}
