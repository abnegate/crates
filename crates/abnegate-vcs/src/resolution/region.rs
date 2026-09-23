#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Region {
    Outside,
    Ours,
    Base,
    Theirs,
}
