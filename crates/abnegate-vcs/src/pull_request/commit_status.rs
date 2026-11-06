/// The latest status one context reported on a commit, its state in GitHub's
/// own words.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommitStatus {
    /// What reported it, such as `ci/deploy`.
    pub context: String,
    /// What it says, such as `pending`, `success`, `failure` or `error`.
    pub state: String,
    /// What the reporter wrote about it, if anything.
    pub description: Option<String>,
    /// Where the reporter shows its details, if it says.
    pub target_url: Option<String>,
    /// When it was reported, as RFC 3339, if GitHub says.
    pub created_at: Option<String>,
}

impl CommitStatus {
    /// `context` reporting `state`, with nothing else said about it until the
    /// `with_*` methods say otherwise.
    pub fn new(context: impl Into<String>, state: impl Into<String>) -> Self {
        Self {
            context: context.into(),
            state: state.into(),
            description: None,
            target_url: None,
            created_at: None,
        }
    }

    /// What the reporter wrote about it.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Where the reporter shows its details.
    #[must_use]
    pub fn with_target_url(mut self, target_url: impl Into<String>) -> Self {
        self.target_url = Some(target_url.into());
        self
    }

    /// When it was reported, as RFC 3339.
    #[must_use]
    pub fn with_created_at(mut self, created_at: impl Into<String>) -> Self {
        self.created_at = Some(created_at.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_builder_sets_its_own_field_and_no_other() {
        let bare = CommitStatus::new("ci/deploy", "success");

        for (built, expected) in [
            (
                bare.clone().with_description("Deployed"),
                CommitStatus {
                    description: Some("Deployed".to_string()),
                    ..bare.clone()
                },
            ),
            (
                bare.clone().with_target_url("https://ci.example/7"),
                CommitStatus {
                    target_url: Some("https://ci.example/7".to_string()),
                    ..bare.clone()
                },
            ),
            (
                bare.clone().with_created_at("2026-09-28T02:51:09Z"),
                CommitStatus {
                    created_at: Some("2026-09-28T02:51:09Z".to_string()),
                    ..bare.clone()
                },
            ),
        ] {
            assert_eq!(built, expected);
        }
        assert_eq!(
            (bare.context.as_str(), bare.state.as_str()),
            ("ci/deploy", "success")
        );
    }
}
