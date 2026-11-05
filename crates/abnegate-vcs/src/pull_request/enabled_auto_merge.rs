use crate::pull_request::MergeMethod;

/// Auto-merge as it is set on a pull request: it merges this way once
/// everything its base requires is met.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EnabledAutoMerge {
    /// How its commits will land on the base, or `None` when GitHub names a
    /// method this crate does not know.
    pub method: Option<MergeMethod>,
    /// Who set it, if GitHub says.
    pub enabled_by: Option<String>,
    /// When it was set, as GitHub writes the time, if GitHub says.
    pub enabled_at: Option<String>,
    /// The merge commit's headline, if one was given.
    pub title: Option<String>,
    /// The merge commit's body, if one was given.
    pub message: Option<String>,
}

impl EnabledAutoMerge {
    /// Auto-merge set to merge with `method`, by nobody GitHub names, at no
    /// time it says, with no headline or body, until the `with_*` methods say
    /// otherwise.
    pub fn new(method: Option<MergeMethod>) -> Self {
        Self {
            method,
            enabled_by: None,
            enabled_at: None,
            title: None,
            message: None,
        }
    }

    /// Who set it.
    #[must_use]
    pub fn with_enabled_by(mut self, login: impl Into<String>) -> Self {
        self.enabled_by = Some(login.into());
        self
    }

    /// When it was set.
    #[must_use]
    pub fn with_enabled_at(mut self, at: impl Into<String>) -> Self {
        self.enabled_at = Some(at.into());
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
