/// One comment in a review thread.
#[derive(Debug, Clone, PartialEq, Eq)]
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
