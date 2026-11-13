use crate::pull_request::MakeLatest;

/// A release to create: the tag it is published under and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReleaseRequest {
    /// The tag the release is published under, created by GitHub from
    /// [`Self::target`] if it does not exist yet.
    pub tag: String,
    /// The branch or commit the tag is created from if it does not exist yet,
    /// or `None` for the repository's default branch.
    pub target: Option<String>,
    /// The release's title, or `None` for GitHub to title it by its tag.
    pub name: Option<String>,
    /// The release's notes, in Markdown, or `None` for none.
    pub body: Option<String>,
    /// Whether it is created as a draft, seen only by those who can push.
    pub draft: bool,
    /// Whether it is marked as not ready for production.
    pub prerelease: bool,
    /// Whether it is marked as the repository's latest release, or `None`
    /// for GitHub's own default.
    pub make_latest: Option<MakeLatest>,
}

impl ReleaseRequest {
    /// A published release under `tag`, from the default branch if the tag
    /// does not exist yet, untitled, with no notes, no prerelease, and marked
    /// latest as GitHub decides, until the `with_*` methods say otherwise.
    pub fn new(tag: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            target: None,
            name: None,
            body: None,
            draft: false,
            prerelease: false,
            make_latest: None,
        }
    }

    /// Create the tag from `target`, a branch or commit, if it does not exist
    /// yet.
    #[must_use]
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Title the release `name`.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Give the release the notes `body`, in Markdown.
    #[must_use]
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Whether to create it as a draft.
    #[must_use]
    pub fn with_draft(mut self, draft: bool) -> Self {
        self.draft = draft;
        self
    }

    /// Whether to mark it as not ready for production.
    #[must_use]
    pub fn with_prerelease(mut self, prerelease: bool) -> Self {
        self.prerelease = prerelease;
        self
    }

    /// Whether to mark it as the repository's latest release.
    #[must_use]
    pub fn with_make_latest(mut self, make_latest: MakeLatest) -> Self {
        self.make_latest = Some(make_latest);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_request_is_a_plain_published_release_until_told() {
        let request = ReleaseRequest::new("v1.2.0");

        assert_eq!(request.tag, "v1.2.0");
        assert_eq!(request.target, None);
        assert_eq!(request.name, None);
        assert_eq!(request.body, None);
        assert!(!request.draft);
        assert!(!request.prerelease);
        assert_eq!(request.make_latest, None);
    }

    #[test]
    fn each_builder_sets_its_own_field_and_no_other() {
        let built = ReleaseRequest::new("v1.2.0")
            .with_target("main")
            .with_name("Version 1.2")
            .with_body("Adds basket totals.")
            .with_draft(true)
            .with_prerelease(true)
            .with_make_latest(MakeLatest::False);

        assert_eq!(
            built,
            ReleaseRequest {
                target: Some("main".to_string()),
                name: Some("Version 1.2".to_string()),
                body: Some("Adds basket totals.".to_string()),
                draft: true,
                prerelease: true,
                make_latest: Some(MakeLatest::False),
                ..ReleaseRequest::new("v1.2.0")
            }
        );
    }
}
