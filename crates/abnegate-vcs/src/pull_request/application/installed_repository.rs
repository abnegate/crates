use crate::branch_name::BranchName;
use crate::pull_request::Repository;

/// A repository a GitHub App installation can reach, as GitHub lists it.
///
/// `owner` and `name` hold only the characters GitHub allows in them, the
/// same rules [`Repository::new`] applies, and `full_name` is derived from
/// that validated owner and name rather than taken from GitHub's answer, so
/// it is always `owner/name`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InstalledRepository {
    /// GitHub's identifier for the repository.
    pub id: u64,
    /// The account or organisation the repository belongs to.
    pub owner: String,
    /// The repository's name, without `.git`.
    pub name: String,
    /// The owner and name joined by `/`, derived from the two above.
    pub full_name: String,
    /// Whether only those granted access can see it.
    pub private: bool,
    /// Whether it is archived, and so read-only.
    pub archived: bool,
    /// The branch it is checked out on unless another is asked for.
    pub default_branch: BranchName,
    /// Where it can be read in a browser.
    pub html_url: String,
}

impl InstalledRepository {
    /// `repository`, which GitHub identifies by `id`, defaults to
    /// `default_branch` and is readable at `html_url`, neither private nor
    /// archived.
    pub fn new(
        id: u64,
        repository: &Repository,
        default_branch: BranchName,
        html_url: impl Into<String>,
    ) -> Self {
        Self {
            id,
            owner: repository.owner().to_string(),
            name: repository.name().to_string(),
            full_name: repository.to_string(),
            private: false,
            archived: false,
            default_branch,
            html_url: html_url.into(),
        }
    }

    /// Whether only those granted access can see it.
    #[must_use]
    pub fn with_private(mut self, private: bool) -> Self {
        self.private = private;
        self
    }

    /// Whether it is archived.
    #[must_use]
    pub fn with_archived(mut self, archived: bool) -> Self {
        self.archived = archived;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed() -> InstalledRepository {
        InstalledRepository::new(
            42,
            &Repository::new("acme", "project").unwrap(),
            BranchName::parse("main").unwrap(),
            "https://github.com/acme/project",
        )
    }

    #[test]
    fn a_new_installed_repository_takes_its_names_from_the_repository() {
        let repository = installed();

        assert_eq!(repository.id, 42);
        assert_eq!(repository.owner, "acme");
        assert_eq!(repository.name, "project");
        assert_eq!(repository.full_name, "acme/project");
        assert_eq!(repository.default_branch.as_ref(), "main");
        assert_eq!(repository.html_url, "https://github.com/acme/project");
    }

    #[test]
    fn a_new_installed_repository_is_neither_private_nor_archived() {
        let repository = installed();

        assert!(!repository.private);
        assert!(!repository.archived);
    }

    #[test]
    fn with_private_marks_it_private_and_nothing_else() {
        let repository = installed().with_private(true);

        assert!(repository.private);
        assert!(!repository.archived);
        assert_eq!(repository.with_private(false), installed());
    }

    #[test]
    fn with_archived_marks_it_archived_and_nothing_else() {
        let repository = installed().with_archived(true);

        assert!(repository.archived);
        assert!(!repository.private);
        assert_eq!(repository.with_archived(false), installed());
    }
}
