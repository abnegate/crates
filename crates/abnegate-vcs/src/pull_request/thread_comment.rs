/// One comment in a review thread.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ThreadComment {
    /// The comment's REST identifier, which a reply is addressed to, when
    /// GitHub gives one.
    pub database_id: Option<u64>,
    /// The login of whoever wrote it, or empty when GitHub does not say.
    pub author: String,
    /// What it says, in Markdown.
    pub body: String,
    /// Where it can be read in a browser.
    pub url: String,
    /// When it was written, as RFC 3339.
    pub created_at: String,
}

impl ThreadComment {
    /// A comment written by `author` at `created_at`, as RFC 3339, saying
    /// `body` and readable at `url`, which a reply is addressed to by
    /// `database_id` when GitHub gives one.
    pub fn new(
        database_id: Option<u64>,
        author: impl Into<String>,
        body: impl Into<String>,
        url: impl Into<String>,
        created_at: impl Into<String>,
    ) -> Self {
        Self {
            database_id,
            author: author.into(),
            body: body.into(),
            url: url.into(),
            created_at: created_at.into(),
        }
    }
}
