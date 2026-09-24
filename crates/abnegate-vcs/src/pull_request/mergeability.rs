/// Whether a pull request's branch still merges with its base.
///
/// GitHub computes this in the background and answers `null` until it has,
/// which is a third answer rather than a missing one: a branch nobody has
/// checked yet is not a branch known to conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Mergeability {
    /// The branch merges with its base.
    Clean,
    /// The branch conflicts with its base.
    Conflicted,
    /// GitHub has not checked yet.
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

    /// Whether GitHub has yet to check the branch. Until it has, a merge is
    /// refused with the same answer a branch protection rule gives.
    pub fn unknown(self) -> bool {
        self == Mergeability::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_nobody_has_checked_is_unknown_and_not_conflicted() {
        let unchecked = Mergeability::from_flag(None);
        assert!(unchecked.unknown());
        assert!(!unchecked.conflicted());

        let clean = Mergeability::from_flag(Some(true));
        assert!(!clean.unknown());
        assert!(!clean.conflicted());

        let conflicted = Mergeability::from_flag(Some(false));
        assert!(!conflicted.unknown());
        assert!(conflicted.conflicted());
    }
}
