use crate::branch_name::BranchName;
use crate::pull_request::Repository;

/// A repository GitHub just created.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CreatedRepository {
    /// Its owner and name, as GitHub recorded them.
    pub repository: Repository,
    /// Where it can be read in a browser.
    pub url: String,
    /// The HTTPS URL it clones from.
    pub clone_url: String,
    /// The branch its first commit landed on.
    pub default_branch: BranchName,
}
