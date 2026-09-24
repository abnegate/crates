/// A comment on a pull request's conversation, where review bots leave their
/// summaries.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IssueComment {
    /// GitHub's identifier for the comment.
    pub id: u64,
    /// The login of whoever wrote it, or empty when GitHub does not say.
    pub author: String,
    /// What it says, in Markdown, or empty when it says nothing.
    pub body: String,
    /// Where it can be read in a browser.
    pub url: String,
    /// When it was written, as RFC 3339.
    pub created_at: String,
}

impl IssueComment {
    /// The comment GitHub identifies by `id`, written by `author` at
    /// `created_at`, as RFC 3339, saying `body` and readable at `url`.
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
            url: url.into(),
            created_at: created_at.into(),
        }
    }
}
