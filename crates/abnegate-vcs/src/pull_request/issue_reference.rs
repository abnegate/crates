use crate::pull_request::PullRequestReference;
use crate::pull_request::Repository;
use std::num::NonZeroU64;

/// Where an issue lives, made with [`Self::new`] from a repository and a
/// number, or from a [`PullRequestReference`], since GitHub numbers issues and
/// pull requests from one sequence and answers for a pull request's
/// conversation as an issue.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IssueReference {
    repository: Repository,
    number: NonZeroU64,
}

impl IssueReference {
    /// Issue `number` in `repository`.
    pub fn new(repository: Repository, number: NonZeroU64) -> Self {
        Self { repository, number }
    }

    /// The repository it lives in.
    pub fn repository(&self) -> &Repository {
        &self.repository
    }

    /// Its number in that repository.
    pub fn number(&self) -> NonZeroU64 {
        self.number
    }
}

impl From<&PullRequestReference> for IssueReference {
    fn from(reference: &PullRequestReference) -> Self {
        Self::new(reference.repository().clone(), reference.number())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_request_is_addressed_as_the_issue_of_the_same_number() {
        let repository = Repository::new("acme", "project").unwrap();
        let number = NonZeroU64::new(7).unwrap();

        let issue = IssueReference::from(&PullRequestReference::new(repository.clone(), number));

        assert_eq!(issue, IssueReference::new(repository.clone(), number));
        assert_eq!(issue.repository(), &repository);
        assert_eq!(issue.number(), number);
    }
}
