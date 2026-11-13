/// What to ask GitHub to write release notes for: a tag, the branch or
/// commit it is or will be created from, and the release to write them since.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NotesRequest {
    /// The tag the release is, or will be, published under.
    pub tag: String,
    /// The branch or commit the tag is created from if it does not exist yet.
    pub target: String,
    /// The tag of the release to write the notes since, or `None` for GitHub
    /// to choose the latest release before `tag`.
    pub previous_tag: Option<String>,
}

impl NotesRequest {
    /// Notes for the release under `tag`, created from `target` if it does
    /// not exist yet, since the release GitHub chooses as the one before it.
    pub fn new(tag: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            target: target.into(),
            previous_tag: None,
        }
    }

    /// Write the notes since the release under `previous_tag`.
    #[must_use]
    pub fn with_previous_tag(mut self, previous_tag: impl Into<String>) -> Self {
        self.previous_tag = Some(previous_tag.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_leaves_the_previous_release_to_github_until_told() {
        let request = NotesRequest::new("v1.2.0", "main");

        assert_eq!(request.tag, "v1.2.0");
        assert_eq!(request.target, "main");
        assert_eq!(request.previous_tag, None);
        assert_eq!(
            request.with_previous_tag("v1.1.0").previous_tag.as_deref(),
            Some("v1.1.0")
        );
    }
}
