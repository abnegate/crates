use crate::pull_request::IssueReference;
use crate::pull_request::IssueState;
use crate::pull_request::Label;

/// An issue as GitHub describes it, or a pull request read as the issue
/// GitHub also answers for it, which [`Self::pull_request`] says.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Issue {
    /// Where it lives.
    pub reference: IssueReference,
    /// Its title.
    pub title: String,
    /// What it says, in Markdown and in full, or `None` when it says nothing.
    pub body: Option<String>,
    /// Whether it is still open.
    pub state: IssueState,
    /// Where it can be read in a browser.
    pub url: String,
    /// The login of whoever opened it, or `None` when GitHub does not say,
    /// as for an account that was deleted.
    pub author: Option<String>,
    /// The labels on it, in the order GitHub listed them.
    pub labels: Vec<Label>,
    /// The logins of those it is assigned to, or `None` when GitHub does not
    /// say.
    pub assignees: Option<Vec<String>>,
    /// How many comments its conversation holds.
    pub comments: u64,
    /// When it was opened, as RFC 3339.
    pub created_at: String,
    /// When it last changed, as RFC 3339.
    pub updated_at: String,
    /// When it was closed, as RFC 3339, or `None` while it is open.
    pub closed_at: Option<String>,
    /// Whether it is a pull request rather than an issue.
    pub pull_request: bool,
}

impl Issue {
    /// The open issue at `reference` titled `title`, readable at `url`,
    /// opened at `created_at` and last changed at `updated_at`, both as
    /// RFC 3339, with no labels or comments and nothing else said of it.
    pub fn new(
        reference: IssueReference,
        title: impl Into<String>,
        url: impl Into<String>,
        created_at: impl Into<String>,
        updated_at: impl Into<String>,
    ) -> Self {
        Self {
            reference,
            title: title.into(),
            body: None,
            state: IssueState::Open,
            url: url.into(),
            author: None,
            labels: Vec::new(),
            assignees: None,
            comments: 0,
            created_at: created_at.into(),
            updated_at: updated_at.into(),
            closed_at: None,
            pull_request: false,
        }
    }

    /// What it says, in Markdown.
    #[must_use]
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Whether it is still open.
    #[must_use]
    pub fn with_state(mut self, state: IssueState) -> Self {
        self.state = state;
        self
    }

    /// The login of whoever opened it.
    #[must_use]
    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
        self
    }

    /// The labels on it.
    #[must_use]
    pub fn with_labels(mut self, labels: impl IntoIterator<Item = Label>) -> Self {
        self.labels = labels.into_iter().collect();
        self
    }

    /// The logins of those it is assigned to.
    #[must_use]
    pub fn with_assignees(
        mut self,
        assignees: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.assignees = Some(assignees.into_iter().map(Into::into).collect());
        self
    }

    /// How many comments its conversation holds.
    #[must_use]
    pub fn with_comments(mut self, comments: u64) -> Self {
        self.comments = comments;
        self
    }

    /// When it was closed, as RFC 3339.
    #[must_use]
    pub fn with_closed_at(mut self, closed_at: impl Into<String>) -> Self {
        self.closed_at = Some(closed_at.into());
        self
    }

    /// Whether it is a pull request rather than an issue.
    #[must_use]
    pub fn with_pull_request(mut self, pull_request: bool) -> Self {
        self.pull_request = pull_request;
        self
    }
}
