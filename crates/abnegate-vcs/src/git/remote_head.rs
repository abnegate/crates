use crate::branch_name::BranchName;
use crate::commit_sha::CommitSha;

/// A remote's default branch and the commit at its tip, as the remote itself
/// reports them: what a run starts from is asked of the repository, not read
/// from a ref every run of it shares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteHead {
    /// The branch the remote names as its default.
    pub branch: BranchName,
    /// The commit at that branch's tip.
    pub commit: CommitSha,
}
