/// A comment on a pull request's conversation, where review bots leave their
/// summaries.
#[derive(Debug, Clone, PartialEq, Eq)]
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
