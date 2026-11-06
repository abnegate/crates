use crate::branch_name::BranchName;
use crate::commit_sha::CommitSha;
use crate::pull_request::Branch;
use crate::pull_request::PullRequestResult;
use crate::pull_request::github_commit::GitHubCommit;
use serde::Deserialize;

/// A branch as the GitHub REST API lists a repository's branches.
///
/// Everything read of it is required, as GitHub sends it on every branch, so
/// an answer that leaves any of it out is unreadable rather than a branch with
/// something made up in its place.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubRepositoryBranch {
    pub(super) name: String,
    pub(super) commit: GitHubCommit,
    pub(super) protected: bool,
}

impl GitHubRepositoryBranch {
    /// The branch this row describes; a name or commit this crate does not
    /// accept is [`crate::pull_request::PullRequestError::Parse`].
    pub(super) fn branch(self) -> PullRequestResult<Branch> {
        Ok(Branch::new(
            BranchName::parse(&self.name)?,
            CommitSha::parse(&self.commit.sha)?,
            self.protected,
        ))
    }
}
