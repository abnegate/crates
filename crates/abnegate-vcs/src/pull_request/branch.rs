use crate::commit_sha::CommitSha;
use crate::remote_branch_name::RemoteBranchName;

/// A branch of a repository, the commit at its tip, and whether branch
/// protection guards it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Branch {
    /// The branch's name, without `refs/heads/`, as GitHub spells it. One a
    /// git command line could misread, such as `+release`, is listed too;
    /// [`RemoteBranchName::to_branch_name`] refuses it.
    pub name: RemoteBranchName,
    /// The commit at its tip.
    pub commit: CommitSha,
    /// Whether a branch protection rule guards it.
    pub protected: bool,
}

impl Branch {
    /// The branch `name`, whose tip is `commit`, guarded by branch protection
    /// when `protected`.
    pub fn new(name: RemoteBranchName, commit: CommitSha, protected: bool) -> Self {
        Self {
            name,
            commit,
            protected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_holds_what_it_was_made_with() {
        let name = RemoteBranchName::parse("+release").unwrap();
        let commit = CommitSha::parse(&"a".repeat(40)).unwrap();

        let branch = Branch::new(name.clone(), commit.clone(), true);

        assert_eq!(branch.name, name);
        assert_eq!(branch.commit, commit);
        assert!(branch.protected);
    }
}
