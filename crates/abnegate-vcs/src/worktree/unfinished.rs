/// What removing a worktree would lose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Unfinished {
    /// Changes in the working tree or the index that no commit holds.
    pub uncommitted: bool,
    /// HEAD is not a commit the caller knows to be safe — neither the one the
    /// run started on nor one that was pushed — so it holds work that was
    /// committed and never published.
    pub unpublished: bool,
}

impl Unfinished {
    /// Whether removing the worktree would lose anything.
    pub fn any(self) -> bool {
        self.uncommitted || self.unpublished
    }
}
