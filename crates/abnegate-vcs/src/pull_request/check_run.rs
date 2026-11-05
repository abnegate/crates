use crate::commit_sha::CommitSha;

/// One check run on a commit, its status and conclusion in GitHub's own words.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CheckRun {
    /// GitHub's identifier for the run, if it says.
    pub id: Option<u64>,
    /// What the run is called, if GitHub says.
    pub name: Option<String>,
    /// The commit the run checked, if GitHub says.
    pub head_sha: Option<CommitSha>,
    /// Where the run is in its life, such as `queued`, `in_progress` or
    /// `completed`.
    pub status: String,
    /// How the run finished, such as `success` or `failure`, or `None` while
    /// it has not.
    pub conclusion: Option<String>,
    /// Where the run can be read on GitHub, if it says.
    pub url: Option<String>,
    /// Where the integration that ran it shows its details, if it says.
    pub details_url: Option<String>,
    /// When the run started, as RFC 3339, if it has.
    pub started_at: Option<String>,
    /// When the run finished, as RFC 3339, if it has.
    pub completed_at: Option<String>,
}

impl CheckRun {
    /// A run at `status`, with nothing else GitHub says about it until the
    /// `with_*` methods say otherwise.
    pub fn new(status: impl Into<String>) -> Self {
        Self {
            id: None,
            name: None,
            head_sha: None,
            status: status.into(),
            conclusion: None,
            url: None,
            details_url: None,
            started_at: None,
            completed_at: None,
        }
    }

    /// GitHub's identifier for the run.
    #[must_use]
    pub fn with_id(mut self, id: u64) -> Self {
        self.id = Some(id);
        self
    }

    /// What the run is called.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The commit the run checked.
    #[must_use]
    pub fn with_head_sha(mut self, head_sha: CommitSha) -> Self {
        self.head_sha = Some(head_sha);
        self
    }

    /// How the run finished.
    #[must_use]
    pub fn with_conclusion(mut self, conclusion: impl Into<String>) -> Self {
        self.conclusion = Some(conclusion.into());
        self
    }

    /// Where the run can be read on GitHub.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// Where the integration that ran it shows its details.
    #[must_use]
    pub fn with_details_url(mut self, details_url: impl Into<String>) -> Self {
        self.details_url = Some(details_url.into());
        self
    }

    /// When the run started, as RFC 3339.
    #[must_use]
    pub fn with_started_at(mut self, started_at: impl Into<String>) -> Self {
        self.started_at = Some(started_at.into());
        self
    }

    /// When the run finished, as RFC 3339.
    #[must_use]
    pub fn with_completed_at(mut self, completed_at: impl Into<String>) -> Self {
        self.completed_at = Some(completed_at.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head() -> CommitSha {
        CommitSha::parse(&"a".repeat(40)).unwrap()
    }

    #[test]
    fn each_builder_sets_its_own_field_and_no_other() {
        let bare = CheckRun::new("completed");

        for (built, expected) in [
            (
                bare.clone().with_id(7),
                CheckRun {
                    id: Some(7),
                    ..bare.clone()
                },
            ),
            (
                bare.clone().with_name("build"),
                CheckRun {
                    name: Some("build".to_string()),
                    ..bare.clone()
                },
            ),
            (
                bare.clone().with_head_sha(head()),
                CheckRun {
                    head_sha: Some(head()),
                    ..bare.clone()
                },
            ),
            (
                bare.clone().with_conclusion("success"),
                CheckRun {
                    conclusion: Some("success".to_string()),
                    ..bare.clone()
                },
            ),
            (
                bare.clone()
                    .with_url("https://github.com/acme/project/runs/7"),
                CheckRun {
                    url: Some("https://github.com/acme/project/runs/7".to_string()),
                    ..bare.clone()
                },
            ),
            (
                bare.clone().with_details_url("https://ci.example/7"),
                CheckRun {
                    details_url: Some("https://ci.example/7".to_string()),
                    ..bare.clone()
                },
            ),
            (
                bare.clone().with_started_at("2026-09-28T02:51:09Z"),
                CheckRun {
                    started_at: Some("2026-09-28T02:51:09Z".to_string()),
                    ..bare.clone()
                },
            ),
            (
                bare.clone().with_completed_at("2026-09-28T02:52:09Z"),
                CheckRun {
                    completed_at: Some("2026-09-28T02:52:09Z".to_string()),
                    ..bare.clone()
                },
            ),
        ] {
            assert_eq!(built, expected);
        }
        assert_eq!(bare.status, "completed");
    }
}
