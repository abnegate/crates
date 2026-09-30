use crate::commit_sha::CommitSha;
use crate::conflict::layout::Layout;
use tempfile::TempDir;

/// A throwaway repository holding both sides of a request, with the head
/// checked out detached.
pub(super) struct Fetched {
    pub(super) root: TempDir,
    pub(super) layout: Layout,
    pub(super) head: CommitSha,
    pub(super) base: CommitSha,
}
