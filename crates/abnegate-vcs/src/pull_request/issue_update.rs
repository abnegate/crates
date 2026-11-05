use crate::pull_request::IssueState;

/// What to change on an issue: only what is set is sent, so everything else
/// stays as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssueUpdate {
    pub(super) title: Option<String>,
    pub(super) body: Option<String>,
    pub(super) state: Option<IssueState>,
    pub(super) labels: Option<Vec<String>>,
}

impl IssueUpdate {
    /// Retitle it `title`.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Make it say `body`, in Markdown.
    #[must_use]
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Open or close it. [`IssueState::Unknown`] names no state GitHub
    /// takes, so it sends none and leaves the issue as open or closed as it
    /// was.
    #[must_use]
    pub fn with_state(mut self, state: IssueState) -> Self {
        self.state = (state != IssueState::Unknown).then_some(state);
        self
    }

    /// Replace its labels with those named `labels`; none at all clears them.
    #[must_use]
    pub fn with_labels(mut self, labels: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.labels = Some(labels.into_iter().map(Into::into).collect());
        self
    }
}
