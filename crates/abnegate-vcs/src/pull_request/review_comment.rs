/// A comment on a pull request's diff, read whole by the identifier a reply
/// to it is addressed to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReviewComment {
    /// GitHub's REST identifier for the comment.
    pub id: u64,
    /// The login of whoever wrote it, or empty when GitHub does not say.
    pub author: String,
    /// What it says, in Markdown, or empty when it says nothing.
    pub body: String,
    /// The file it was left on, if GitHub says.
    pub path: Option<String>,
    /// Where it can be read in a browser.
    pub url: String,
    /// When it was written, as RFC 3339.
    pub created_at: String,
}

impl ReviewComment {
    /// The review comment GitHub identifies by `id`, written by `author` at
    /// `created_at`, as RFC 3339, saying `body` and readable at `url`, on no
    /// file GitHub says.
    pub fn new(
        id: u64,
        author: impl Into<String>,
        body: impl Into<String>,
        url: impl Into<String>,
        created_at: impl Into<String>,
    ) -> Self {
        Self {
            id,
            author: author.into(),
            body: body.into(),
            path: None,
            url: url.into(),
            created_at: created_at.into(),
        }
    }

    /// The file it was left on.
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written() -> ReviewComment {
        ReviewComment::new(
            99,
            "review-bot",
            "handle the empty cart",
            "https://github.com/acme/project/pull/7#discussion_r99",
            "2026-09-20T10:00:00Z",
        )
    }

    #[test]
    fn a_review_comment_is_built_on_no_file() {
        let comment = written();

        assert_eq!(comment.id, 99);
        assert_eq!(comment.author, "review-bot");
        assert_eq!(comment.body, "handle the empty cart");
        assert_eq!(comment.path, None);
        assert_eq!(
            comment.url,
            "https://github.com/acme/project/pull/7#discussion_r99"
        );
        assert_eq!(comment.created_at, "2026-09-20T10:00:00Z");
    }

    #[test]
    fn a_path_names_the_file_and_nothing_else() {
        let comment = written().with_path("src/cart.ts");

        assert_eq!(comment.path.as_deref(), Some("src/cart.ts"));
        assert_eq!(
            ReviewComment {
                path: None,
                ..comment
            },
            written()
        );
    }
}
