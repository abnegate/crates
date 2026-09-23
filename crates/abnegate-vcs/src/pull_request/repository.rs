use std::fmt;

/// A repository on the host a pull request service answers for, as an owner
/// and a name made only of the characters GitHub allows in them. Only
/// [`crate::pull_request::PullRequestService::parse_github_url`] makes one, so
/// every request path built from it addresses the repository it was parsed
/// from and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Repository {
    owner: String,
    name: String,
}

impl Repository {
    pub(super) fn new(owner: &str, name: &str) -> Option<Self> {
        (named(owner) && named(name)).then(|| Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

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
fn named(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}
