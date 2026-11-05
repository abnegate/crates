use crate::pull_request::PullRequestError;
use crate::pull_request::PullRequestResult;
use std::fmt;

/// A repository on the host a pull request service answers for, as an owner
/// and a name made only of the characters GitHub allows in them, so every
/// request path built from it addresses that repository and nothing else.
///
/// [`crate::pull_request::PullRequestService`] makes one from a URL it parsed
/// or from GitHub's answer to creating the repository, and a caller that
/// already has the owner and name makes one with [`Self::new`], or with
/// [`crate::pull_request::PullRequestService::repository_on`] to hold it to
/// the host the service answers for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Repository {
    owner: String,
    name: String,
}

impl Repository {
    /// The repository `name` belonging to `owner`.
    ///
    /// Either one holding a character GitHub does not allow in it, or being
    /// empty, `.` or `..`, is [`PullRequestError::InvalidRepositoryName`].
    pub fn new(owner: &str, name: &str) -> PullRequestResult<Self> {
        match named(owner) && named(name) {
            true => Ok(Self {
                owner: owner.to_string(),
                name: name.to_string(),
            }),
            false => Err(PullRequestError::InvalidRepositoryName),
        }
    }

    /// The account or organisation the repository belongs to.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// The repository's name, without `.git`.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for Repository {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.owner, self.name)
    }
}

/// An owner or repository name: what GitHub allows in one, and never a path
/// traversal.
pub(super) fn named(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repository_is_made_from_an_owner_and_name_github_allows() {
        let repository = Repository::new("acme-corp", "my.project_2").unwrap();

        assert_eq!(repository.owner(), "acme-corp");
        assert_eq!(repository.name(), "my.project_2");
        assert_eq!(repository.to_string(), "acme-corp/my.project_2");
    }

    #[test]
    fn an_owner_or_name_github_does_not_allow_is_refused() {
        for (owner, name) in [
            ("", "project"),
            ("acme", ""),
            ("..", "project"),
            ("acme", "."),
            ("acme/other", "project"),
            ("acme", "project?x=1"),
            ("acme", "pro ject"),
            ("acme", "../../user"),
        ] {
            assert!(
                matches!(
                    Repository::new(owner, name),
                    Err(PullRequestError::InvalidRepositoryName)
                ),
                "{owner:?}/{name:?}"
            );
        }
    }
}
