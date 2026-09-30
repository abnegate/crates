use crate::commit_sha::CommitSha;

/// What a refresh left at the head of the branch.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Refreshed {
    /// The commit the branch is at: the merge that was pushed, or the head
    /// that was fetched when there was nothing to merge.
    pub commit: CommitSha,
    /// Whether a merge was made and pushed; false when the base was already
    /// contained in the head.
    pub pushed: bool,
}

impl Refreshed {
    /// A branch left at `commit`, by a push of it when `pushed` says so.
    pub fn new(commit: CommitSha, pushed: bool) -> Self {
        Self { commit, pushed }
    }
}
