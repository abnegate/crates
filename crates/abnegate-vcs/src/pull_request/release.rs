/// A release published on a repository, as GitHub describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Release {
    /// GitHub's identifier for the release.
    pub id: u64,
    /// The tag it was published under.
    pub tag: String,
    /// The title it was given, or `None` when it has none.
    pub name: Option<String>,
    /// Where it can be read in a browser.
    pub url: String,
    /// When it was published, as RFC 3339, or `None` while it is a draft.
    pub published_at: Option<String>,
    /// Whether it is a draft, seen only by those who can push.
    pub draft: bool,
    /// Whether it is marked as not ready for production.
    pub prerelease: bool,
    /// Its notes, in Markdown, or `None` when it has none.
    pub body: Option<String>,
    /// The branch or commit its tag was, or will be, created from, or `None`
    /// when GitHub does not say.
    pub target: Option<String>,
}

impl Release {
    /// The release GitHub identifies by `id` under `tag`, readable at `url`,
    /// neither a draft nor a prerelease, with nothing else said of it.
    pub fn new(id: u64, tag: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id,
            tag: tag.into(),
            name: None,
            url: url.into(),
            published_at: None,
            draft: false,
            prerelease: false,
            body: None,
            target: None,
        }
    }

    /// The title it was given.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// When it was published, as RFC 3339.
    #[must_use]
    pub fn with_published_at(mut self, published_at: impl Into<String>) -> Self {
        self.published_at = Some(published_at.into());
        self
    }

    /// Whether it is a draft.
    #[must_use]
    pub fn with_draft(mut self, draft: bool) -> Self {
        self.draft = draft;
        self
    }

    /// Whether it is marked as not ready for production.
    #[must_use]
    pub fn with_prerelease(mut self, prerelease: bool) -> Self {
        self.prerelease = prerelease;
        self
    }

    /// Its notes, in Markdown.
    #[must_use]
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// The branch or commit its tag was, or will be, created from.
    #[must_use]
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_builder_sets_its_own_field_and_no_other() {
        let base = Release::new(
            7,
            "v1.0.0",
            "https://github.com/acme/project/releases/v1.0.0",
        );

        let built = base
            .clone()
            .with_body("Adds basket totals.")
            .with_target("main");

        assert_eq!(
            built,
            Release {
                body: Some("Adds basket totals.".to_string()),
                target: Some("main".to_string()),
                ..base
            }
        );
    }
}
