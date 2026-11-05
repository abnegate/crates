use crate::commit_sha::CommitSha;
use crate::pull_request::MergeMethod;

/// How a pull request set to merge once it is ready lands on its base: the
/// method, the head it has to still be at, and the commit the merge makes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AutoMerge {
    /// How its commits land on the base.
    pub method: MergeMethod,
    /// The head it has to still be at for auto-merge to be set, or `None` to
    /// take whatever head it has.
    pub head: Option<CommitSha>,
    /// The merge commit's headline, or `None` for GitHub's own.
    pub title: Option<String>,
    /// The merge commit's body, or `None` for GitHub's own.
    pub message: Option<String>,
}

impl AutoMerge {
    /// Merge with `method` at whatever head the pull request has, with the
    /// commit headline and body GitHub writes, until the `with_*` methods say
    /// otherwise.
    pub fn new(method: MergeMethod) -> Self {
        Self {
            method,
            head: None,
            title: None,
            message: None,
        }
    }

    /// Set auto-merge only while the pull request's head is still `head`.
    #[must_use]
    pub fn with_head(mut self, head: CommitSha) -> Self {
        self.head = Some(head);
        self
    }

    /// The merge commit's headline.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// The merge commit's body.
    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}
