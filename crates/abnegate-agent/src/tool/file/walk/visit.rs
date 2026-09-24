/// What a [`Walk`](super::Walk) does after showing its visitor an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tool::file) enum Visit {
    /// Walk into this directory.
    Descend,
    /// Leave this entry and carry on with its siblings.
    Skip,
    /// End the walk here.
    Stop,
}
