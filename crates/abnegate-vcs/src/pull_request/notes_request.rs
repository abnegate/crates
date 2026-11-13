/// What to ask GitHub to write release notes for: a tag, the branch or
/// commit it would be created from, and the release to write them since.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NotesRequest {
    /// The tag the release is, or will be, published under.
    pub tag: String,
    /// The branch or commit the tag would be created from if it does not
    /// exist yet, or `None` for the repository's default branch.
    pub target: Option<String>,
    /// The tag of the release to write the notes since, or `None` for GitHub
    /// to choose the latest release before `tag`.
    pub previous_tag: Option<String>,
}

impl NotesRequest {
    /// Notes for the release under `tag`, created from the default branch if
    /// it does not exist yet, since the release GitHub chooses as the one
    /// before it, until the `with_*` methods say otherwise.
    pub fn new(tag: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            target: None,
            previous_tag: None,
        }
    }

    /// Write the notes as though the tag were created from `target`, a
    /// branch or commit, if it does not exist yet.
    #[must_use]
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
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
    fn a_request_leaves_the_target_and_previous_release_to_github_until_told() {
        let request = NotesRequest::new("v1.2.0");

        assert_eq!(request.tag, "v1.2.0");
        assert_eq!(request.target, None);
        assert_eq!(request.previous_tag, None);
    }

    #[test]
    fn each_builder_sets_its_own_field_and_no_other() {
        let built = NotesRequest::new("v1.2.0")
            .with_target("main")
            .with_previous_tag("v1.1.0");

        assert_eq!(
            built,
            NotesRequest {
                target: Some("main".to_string()),
                previous_tag: Some("v1.1.0".to_string()),
                ..NotesRequest::new("v1.2.0")
            }
        );
    }
}
