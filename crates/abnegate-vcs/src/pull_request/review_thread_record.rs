use crate::pull_request::DiffSide;
use crate::pull_request::ThreadComment;

/// One review thread on a pull request's diff, with the first hundred comments
/// in it.
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
    /// The first line of the range it was left on, when it spans several
    /// and that range is still in the diff.
    pub start_line: Option<u32>,
    /// The line it was left on in the diff it was written against.
    pub original_line: Option<u32>,
    /// The first line of the range it was left on in the diff it was written
    /// against, when it spans several.
    pub original_start_line: Option<u32>,
    /// The side of the diff it was left on.
    pub side: DiffSide,
    /// Its comments, oldest first: at most the first hundred.
    pub comments: Vec<ThreadComment>,
    /// Whether [`Self::comments`] is every comment in the thread: `false`
    /// when GitHub said more follow, or did not say.
    pub comments_complete: bool,
}

impl ReviewThreadRecord {
    /// The thread GitHub's node identifier `id` names, holding `comments`,
    /// oldest first and every comment in it: unresolved, on lines that have
    /// not changed since, and on no file, line or side GitHub says.
    pub fn new(id: impl Into<String>, comments: Vec<ThreadComment>) -> Self {
        Self {
            id: id.into(),
            resolved: false,
            outdated: false,
            path: None,
            line: None,
            start_line: None,
            original_line: None,
            original_start_line: None,
            side: DiffSide::Unknown,
            comments,
            comments_complete: true,
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

    /// The first line of the range it was left on, while that range is still
    /// in the diff.
    #[must_use]
    pub fn with_start_line(mut self, line: u32) -> Self {
        self.start_line = Some(line);
        self
    }

    /// The line it was left on in the diff it was written against.
    #[must_use]
    pub fn with_original_line(mut self, line: u32) -> Self {
        self.original_line = Some(line);
        self
    }

    /// The first line of the range it was left on in the diff it was written
    /// against.
    #[must_use]
    pub fn with_original_start_line(mut self, line: u32) -> Self {
        self.original_start_line = Some(line);
        self
    }

    /// The side of the diff it was left on.
    #[must_use]
    pub fn with_side(mut self, side: DiffSide) -> Self {
        self.side = side;
        self
    }

    /// Whether its comments are every comment in the thread.
    #[must_use]
    pub fn with_comments_complete(mut self, complete: bool) -> Self {
        self.comments_complete = complete;
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
    fn a_thread_built_from_its_identifier_is_unresolved_current_complete_and_on_no_line() {
        assert_eq!(
            opened(),
            ReviewThreadRecord {
                id: "PRRT_1".to_string(),
                resolved: false,
                outdated: false,
                path: None,
                line: None,
                start_line: None,
                original_line: None,
                original_start_line: None,
                side: DiffSide::Unknown,
                comments: vec![ThreadComment {
                    database_id: Some(99),
                    author: "review-bot".to_string(),
                    body: "handle the empty cart".to_string(),
                    url: "https://github.com/acme/project/pull/7#discussion_r99".to_string(),
                    created_at: "2026-09-20T10:00:00Z".to_string(),
                }],
                comments_complete: true,
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
        assert_eq!(
            opened().with_start_line(10),
            ReviewThreadRecord {
                start_line: Some(10),
                ..opened()
            }
        );
        assert_eq!(
            opened().with_original_line(14),
            ReviewThreadRecord {
                original_line: Some(14),
                ..opened()
            }
        );
        assert_eq!(
            opened().with_original_start_line(11),
            ReviewThreadRecord {
                original_start_line: Some(11),
                ..opened()
            }
        );
        assert_eq!(
            opened().with_side(DiffSide::Left),
            ReviewThreadRecord {
                side: DiffSide::Left,
                ..opened()
            }
        );
        assert_eq!(
            opened().with_comments_complete(false),
            ReviewThreadRecord {
                comments_complete: false,
                ..opened()
            }
        );
    }
}
