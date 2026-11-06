use crate::pull_request::Repository;
use crate::remote_branch_name::RemoteBranchName;

/// A repository a GitHub App installation can reach, as GitHub lists it.
///
/// Its owner and name come from a [`Repository`], so they hold only the
/// characters GitHub allows in them, and its full name is derived from that
/// owner and name rather than taken from GitHub's answer, so it is always
/// `owner/name`. Every field is read through an accessor, so nothing can
/// pull the three apart once it is made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRepository {
    id: u64,
    repository: Repository,
    full_name: String,
    private: bool,
    archived: bool,
    default_branch: RemoteBranchName,
    html_url: String,
}

impl InstalledRepository {
    /// `repository`, which GitHub identifies by `id`, defaults to
    /// `default_branch` and is readable at `html_url`, neither private nor
    /// archived.
    pub fn new(
        id: u64,
        repository: Repository,
        default_branch: RemoteBranchName,
        html_url: impl Into<String>,
    ) -> Self {
        Self {
            id,
            full_name: repository.to_string(),
            repository,
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

    /// GitHub's identifier for the repository.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The owner and name, ready to address the repository with
    /// [`crate::pull_request::PullRequestService`].
    pub fn repository(&self) -> &Repository {
        &self.repository
    }

    /// The account or organisation the repository belongs to.
    pub fn owner(&self) -> &str {
        self.repository.owner()
    }

    /// The repository's name, without `.git`.
    pub fn name(&self) -> &str {
        self.repository.name()
    }

    /// The owner and name joined by `/`.
    pub fn full_name(&self) -> &str {
        &self.full_name
    }

    /// Whether only those granted access can see it.
    pub fn private(&self) -> bool {
        self.private
    }

    /// Whether it is archived, and so read-only.
    pub fn archived(&self) -> bool {
        self.archived
    }

    /// The branch it is checked out on unless another is asked for, as GitHub
    /// spells it.
    pub fn default_branch(&self) -> &RemoteBranchName {
        &self.default_branch
    }

    /// Where it can be read in a browser.
    pub fn html_url(&self) -> &str {
        &self.html_url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed() -> InstalledRepository {
        InstalledRepository::new(
            42,
            Repository::new("acme", "project").unwrap(),
            RemoteBranchName::parse("main").unwrap(),
            "https://github.com/acme/project",
        )
    }

    #[test]
    fn a_new_installed_repository_takes_its_names_from_the_repository() {
        let repository = installed();

        assert_eq!(repository.id(), 42);
        assert_eq!(
            repository.repository(),
            &Repository::new("acme", "project").unwrap()
        );
        assert_eq!(repository.owner(), "acme");
        assert_eq!(repository.name(), "project");
        assert_eq!(repository.full_name(), "acme/project");
        assert_eq!(repository.default_branch().as_str(), "main");
        assert_eq!(repository.html_url(), "https://github.com/acme/project");
    }

    #[test]
    fn a_new_installed_repository_is_neither_private_nor_archived() {
        let repository = installed();

        assert!(!repository.private());
        assert!(!repository.archived());
    }

    #[test]
    fn with_private_marks_it_private_and_nothing_else() {
        let repository = installed().with_private(true);

        assert!(repository.private());
        assert!(!repository.archived());
        assert_eq!(repository.with_private(false), installed());
    }

    #[test]
    fn with_archived_marks_it_archived_and_nothing_else() {
        let repository = installed().with_archived(true);

        assert!(repository.archived());
        assert!(!repository.private());
        assert_eq!(repository.with_archived(false), installed());
    }

    #[test]
    fn a_default_branch_a_command_line_would_misread_is_held_as_github_spells_it() {
        let repository = InstalledRepository::new(
            42,
            Repository::new("acme", "project").unwrap(),
            RemoteBranchName::parse("+release").unwrap(),
            "https://github.com/acme/project",
        );

        assert_eq!(repository.default_branch().as_str(), "+release");
        assert!(repository.default_branch().to_branch_name().is_err());
    }
}
