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
}
