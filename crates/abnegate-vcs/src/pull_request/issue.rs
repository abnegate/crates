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
    /// Its title, or empty when GitHub does not say.
    pub title: String,
    /// What it says, in Markdown and in full, or `None` when it says nothing.
    pub body: Option<String>,
    /// Whether it is still open.
    pub state: IssueState,
    /// Where it can be read in a browser, or empty when GitHub does not say.
    pub url: String,
    /// The login of whoever opened it, or empty when GitHub does not say.
    pub author: String,
    /// The labels on it, in the order GitHub listed them.
    pub labels: Vec<Label>,
    /// The logins of those it is assigned to.
    pub assignees: Vec<String>,
    /// How many comments its conversation holds.
    pub comments: u64,
    /// When it was opened, as RFC 3339, or empty when GitHub does not say.
    pub created_at: String,
    /// When it last changed, as RFC 3339, if GitHub says.
    pub updated_at: Option<String>,
    /// When it was closed, as RFC 3339, or `None` while it is open.
    pub closed_at: Option<String>,
    /// Whether it is a pull request rather than an issue.
    pub pull_request: bool,
}

impl Issue {
    /// The open issue at `reference` titled `title`, with nothing else said
    /// of it.
    pub fn new(reference: IssueReference, title: impl Into<String>) -> Self {
        Self {
            reference,
            title: title.into(),
            body: None,
            state: IssueState::Open,
            url: String::new(),
            author: String::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            comments: 0,
            created_at: String::new(),
            updated_at: None,
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

    /// Where it can be read in a browser.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// The login of whoever opened it.
    #[must_use]
    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = author.into();
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
        self.assignees = assignees.into_iter().map(Into::into).collect();
        self
    }

    /// How many comments its conversation holds.
    #[must_use]
    pub fn with_comments(mut self, comments: u64) -> Self {
        self.comments = comments;
        self
    }

    /// When it was opened, as RFC 3339.
    #[must_use]
    pub fn with_created_at(mut self, created_at: impl Into<String>) -> Self {
        self.created_at = created_at.into();
        self
    }

    /// When it last changed, as RFC 3339.
    #[must_use]
    pub fn with_updated_at(mut self, updated_at: impl Into<String>) -> Self {
        self.updated_at = Some(updated_at.into());
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
