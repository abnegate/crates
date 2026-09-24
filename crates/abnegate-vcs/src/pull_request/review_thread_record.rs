use crate::pull_request::ThreadComment;

/// One review thread on a pull request's diff, with every comment in it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReviewThreadRecord {
    /// GitHub's node identifier for the thread, which resolving it takes.
    pub id: String,
    /// Whether someone marked it resolved.
    pub resolved: bool,
    /// Whether the lines it was left on have changed since.
    pub outdated: bool,
    /// The file it was left on, if GitHub says.
    pub path: Option<String>,
    /// The line it was left on, if that line is still in the diff.
    pub line: Option<u32>,
    /// Its comments, oldest first.
    pub comments: Vec<ThreadComment>,
}

impl ReviewThreadRecord {
    /// The thread GitHub's node identifier `id` names, holding `comments`,
    /// oldest first: unresolved, on lines that have not changed since, and on
    /// no file or line GitHub says.
    pub fn new(id: impl Into<String>, comments: Vec<ThreadComment>) -> Self {
        Self {
            id: id.into(),
            resolved: false,
            outdated: false,
            path: None,
            line: None,
            comments,
        }
    }

    /// Whether someone marked it resolved.
    #[must_use]
    pub fn with_resolved(mut self, resolved: bool) -> Self {
        self.resolved = resolved;
        self
    }

    /// Whether the lines it was left on have changed since.
    #[must_use]
    pub fn with_outdated(mut self, outdated: bool) -> Self {
        self.outdated = outdated;
        self
    }

    /// The file it was left on.
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// The line it was left on, while that line is still in the diff.
    #[must_use]
    pub fn with_line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened() -> ReviewThreadRecord {
        ReviewThreadRecord::new(
            "PRRT_1",
            vec![ThreadComment::new(
                Some(99),
                "review-bot",
                "handle the empty cart",
                "https://github.com/acme/project/pull/7#discussion_r99",
                "2026-09-20T10:00:00Z",
            )],
        )
    }

    #[test]
    fn a_thread_built_from_its_identifier_is_unresolved_current_and_on_no_line() {
        assert_eq!(
            opened(),
            ReviewThreadRecord {
                id: "PRRT_1".to_string(),
                resolved: false,
                outdated: false,
                path: None,
                line: None,
                comments: vec![ThreadComment {
                    database_id: Some(99),
                    author: "review-bot".to_string(),
                    body: "handle the empty cart".to_string(),
                    url: "https://github.com/acme/project/pull/7#discussion_r99".to_string(),
                    created_at: "2026-09-20T10:00:00Z".to_string(),
                }],
            }
        );
    }

    #[test]
    fn each_builder_sets_its_own_field_and_no_other() {
        assert_eq!(
            opened().with_resolved(true),
            ReviewThreadRecord {
                resolved: true,
                ..opened()
            }
        );
        assert_eq!(
            opened().with_outdated(true),
            ReviewThreadRecord {
                outdated: true,
                ..opened()
            }
        );
        assert_eq!(
            opened().with_path("src/cart.ts"),
            ReviewThreadRecord {
                path: Some("src/cart.ts".to_string()),
                ..opened()
            }
        );
        assert_eq!(
            opened().with_line(12),
            ReviewThreadRecord {
                line: Some(12),
                ..opened()
            }
        );
    }
}
