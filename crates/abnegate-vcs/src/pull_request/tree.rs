use crate::object_id::ObjectId;
use crate::tree_entry::TreeEntry;

/// A git tree as GitHub lists it: its entries, and whether GitHub left any out.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Tree {
    /// The object GitHub resolved the reference to: a commit when the tree
    /// was read by a branch or a commit, and the tree itself only when it was
    /// read by its own name.
    pub sha: ObjectId,
    /// Its entries, in the order GitHub listed them.
    pub entries: Vec<TreeEntry>,
    /// Whether GitHub cut the list short, past the most entries or bytes it
    /// returns in one answer, so that `entries` does not hold all of them.
    pub truncated: bool,
}

impl Tree {
    /// The tree GitHub resolved to `sha`, holding `entries`, every one of them
    /// listed until [`Self::with_truncated`] says otherwise.
    pub fn new(sha: ObjectId, entries: Vec<TreeEntry>) -> Self {
        Self {
            sha,
            entries,
            truncated: false,
        }
    }

    /// Whether GitHub cut the list of entries short.
    #[must_use]
    pub fn with_truncated(mut self, truncated: bool) -> Self {
        self.truncated = truncated;
        self
    }
}
