use std::path::Path;
use std::path::PathBuf;

/// A checkout git works in, bound to the clone it was made from.
///
/// A checkout is named by its top: the directory its `.git` stands in. A
/// path below the top is inside the same working tree, but it is no
/// checkout of its own, since git run there reaches whichever checkout
/// encloses it, and every operation that takes a `Checkout` refuses it with
/// [`GitError::NotACheckoutTop`](crate::GitError::NotACheckoutTop).
///
/// The clone is the repository whose git directory the checkout's must be:
/// the checkout itself for a base clone, and the clone a linked worktree was
/// added to for one of those. Before git runs, every operation confirms that
/// the git directories git finds from the top are that clone's, and refuses a
/// checkout whose `.git`, or the worktree record it names, leads anywhere
/// else with [`GitError::RedirectedGitDirectory`](crate::GitError::RedirectedGitDirectory),
/// however consistent that record is in itself: another clone's record of a
/// worktree that once stood at the same path names the path as faithfully as
/// the clone's own does.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Checkout {
    top: PathBuf,
    repository: PathBuf,
}

impl Checkout {
    /// A base clone at `top`, whose `.git` directory is its own git
    /// directory and the one every worktree of it shares.
    pub fn base(top: impl Into<PathBuf>) -> Self {
        let top = top.into();
        Self {
            repository: top.clone(),
            top,
        }
    }

    /// A linked worktree at `top` of the clone whose top is `repository`:
    /// its `.git` file must name one of that clone's own worktree records.
    pub fn linked(top: impl Into<PathBuf>, repository: impl Into<PathBuf>) -> Self {
        Self {
            top: top.into(),
            repository: repository.into(),
        }
    }

    /// The top of the checkout, where its `.git` stands.
    pub fn top(&self) -> &Path {
        &self.top
    }

    /// The top of the clone the checkout's git directories must belong to:
    /// the checkout's own top for a base clone.
    pub fn repository(&self) -> &Path {
        &self.repository
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_base_clone_is_its_own_repository() {
        let checkout = Checkout::base("/work/base");

        assert_eq!(checkout.top(), Path::new("/work/base"));
        assert_eq!(checkout.repository(), Path::new("/work/base"));
    }

    #[test]
    fn a_linked_worktree_names_the_clone_it_was_added_to() {
        let checkout = Checkout::linked("/work/base-worktrees/one", "/work/base");

        assert_eq!(checkout.top(), Path::new("/work/base-worktrees/one"));
        assert_eq!(checkout.repository(), Path::new("/work/base"));
    }
}
