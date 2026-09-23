/// Whether a pull request's branch still merges with its base.
///
/// GitHub computes this in the background and answers `null` until it has,
/// which is a third answer rather than a missing one: a branch nobody has
/// checked yet is not a branch known to conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Mergeability {
    Clean,
    Conflicted,
    Unknown,
}

impl Mergeability {
    pub(super) fn from_flag(flag: Option<bool>) -> Self {
        match flag {
            Some(true) => Mergeability::Clean,
            Some(false) => Mergeability::Conflicted,
            None => Mergeability::Unknown,
        }
    }

    /// Whether the branch is known to conflict with its base.
    pub fn conflicted(self) -> bool {
        self == Mergeability::Conflicted
    }
}
