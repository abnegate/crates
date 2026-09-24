use crate::commit_sha::CommitSha;

/// A merge that happened.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MergedPullRequest {
    /// The commit the merge made on the base, or `None` when GitHub named none
    /// this crate could read. A merge without one still happened.
    pub sha: Option<CommitSha>,
    /// Whether it took an administrator's override of branch protection.
    pub administrator: bool,
}
