use crate::pull_request::Repository;
use std::num::NonZeroU64;

/// Where a pull request lives: recovered from the URL a run recorded by
/// [`crate::pull_request::PullRequestService::pull_request`], found by
/// [`crate::pull_request::PullRequestService::find_pull_request`], or made
/// with [`Self::new`] from a repository and a number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestReference {
    repository: Repository,
    number: NonZeroU64,
}

impl PullRequestReference {
    /// Pull request `number` in `repository`.
    pub fn new(repository: Repository, number: NonZeroU64) -> Self {
        Self { repository, number }
    }

    /// The repository it lives in.
    pub fn repository(&self) -> &Repository {
        &self.repository
    }

    /// Its number in that repository.
    pub fn number(&self) -> NonZeroU64 {
        self.number
    }
}
