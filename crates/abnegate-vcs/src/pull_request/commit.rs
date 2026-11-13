use crate::commit_sha::CommitSha;

/// A commit as a pull request lists it: what it is, what it says, and who
/// wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PullRequestCommit {
    /// The commit's full identifier.
    pub sha: CommitSha,
    /// Its whole message, subject and body.
    pub message: String,
    /// The name its author signed it with, or `None` when GitHub does not
    /// say.
    pub author_name: Option<String>,
    /// The login of the GitHub account its author's email belongs to, or
    /// `None` when no account claims it.
    pub author_login: Option<String>,
    /// When its author wrote it, as GitHub's ISO 8601 timestamp, or `None`
    /// when GitHub does not say.
    pub authored_at: Option<String>,
    /// Where it can be read in a browser.
    pub url: String,
}

impl PullRequestCommit {
    /// The commit `sha`, whose message is `message`, readable at `url`.
    ///
    /// Its author's name, login and time are unknown until the `with_*`
    /// methods say otherwise.
    pub fn new(sha: CommitSha, message: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            sha,
            message: message.into(),
            author_name: None,
            author_login: None,
            authored_at: None,
            url: url.into(),
        }
    }

    /// The name its author signed it with.
    #[must_use]
    pub fn with_author_name(mut self, author_name: impl Into<String>) -> Self {
        self.author_name = Some(author_name.into());
        self
    }

    /// The login of the GitHub account its author's email belongs to.
    #[must_use]
    pub fn with_author_login(mut self, author_login: impl Into<String>) -> Self {
        self.author_login = Some(author_login.into());
        self
    }

    /// When its author wrote it, as GitHub's ISO 8601 timestamp.
    #[must_use]
    pub fn with_authored_at(mut self, authored_at: impl Into<String>) -> Self {
        self.authored_at = Some(authored_at.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written() -> PullRequestCommit {
        PullRequestCommit::new(
            CommitSha::parse(&"a".repeat(40)).unwrap(),
            "feat: basket totals",
            "https://github.com/acme/project/commit/aaaa",
        )
    }

    #[test]
    fn a_commit_built_from_what_names_it_has_no_known_author() {
        let built = written();

        assert_eq!(built.message, "feat: basket totals");
        assert_eq!(built.url, "https://github.com/acme/project/commit/aaaa");
        assert_eq!(built.author_name, None);
        assert_eq!(built.author_login, None);
        assert_eq!(built.authored_at, None);
    }

    #[test]
    fn each_builder_sets_its_own_field_and_no_other() {
        let built = written()
            .with_author_name("Mona Lisa")
            .with_author_login("octocat")
            .with_authored_at("2026-11-06T09:30:00Z");

        assert_eq!(
            built,
            PullRequestCommit {
                author_name: Some("Mona Lisa".to_string()),
                author_login: Some("octocat".to_string()),
                authored_at: Some("2026-11-06T09:30:00Z".to_string()),
                ..written()
            }
        );
    }
}
