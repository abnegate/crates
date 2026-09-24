use crate::branch_name::BranchName;
use crate::commit_sha::CommitSha;
use crate::pull_request::Mergeability;
use crate::pull_request::MergeableState;
use crate::pull_request::PullRequestState;
use std::num::NonZeroU64;

/// A pull request as GitHub describes it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestDetail {
    /// GitHub's global node identifier, which its GraphQL API addresses the
    /// pull request by.
    pub node_id: String,
    /// The number it goes by in its repository.
    pub number: NonZeroU64,
    /// Its title.
    pub title: String,
    /// Its body, if it has one.
    pub body: Option<String>,
    /// Whether it is open.
    pub state: PullRequestState,
    /// Whether it is a draft.
    pub draft: bool,
    /// Whether it merged.
    pub merged: bool,
    /// Once merged, the commit the merge made; while open, the test merge
    /// GitHub last prepared. `None` until GitHub has made either.
    pub merge_commit_sha: Option<CommitSha>,
    /// The branch it merges from.
    pub head: BranchName,
    /// The commit at the tip of that branch.
    pub head_sha: CommitSha,
    /// The branch it merges into.
    pub base: BranchName,
    /// Whether the head still merges with the base.
    pub mergeable: Mergeability,
    /// What stands between it and a merge, as GitHub sums it up.
    pub mergeable_state: MergeableState,
    /// Files it changes.
    pub changed_files: u32,
    /// Lines it adds.
    pub additions: u32,
    /// Lines it removes.
    pub deletions: u32,
    /// Commits on its branch.
    pub commits: u32,
    /// Where it can be read in a browser.
    pub url: String,
}
