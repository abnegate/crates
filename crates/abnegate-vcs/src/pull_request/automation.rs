use crate::pull_request::EnabledAutoMerge;
use crate::pull_request::MergeSettings;

/// Whether a pull request is a draft and set to merge once it is ready, what
/// the token may change about that, and what its repository allows.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PullRequestAutomation {
    /// GitHub's global node identifier, which changing any of this takes.
    pub node_id: String,
    /// Whether it is a draft.
    pub draft: bool,
    /// The auto-merge set on it, or `None` when none is.
    pub auto_merge: Option<EnabledAutoMerge>,
    /// Whether the token may set auto-merge on it now.
    pub can_enable_auto_merge: bool,
    /// Whether the token may take its auto-merge off now.
    pub can_disable_auto_merge: bool,
    /// How its repository lets it merge.
    pub settings: MergeSettings,
}

impl PullRequestAutomation {
    /// The pull request GitHub's GraphQL API addresses as `node_id`, in a
    /// repository that lets it merge as `settings` says.
    ///
    /// It is no draft, has no auto-merge set, and the token may change
    /// neither, until the `with_*` methods say otherwise.
    pub fn new(node_id: impl Into<String>, settings: MergeSettings) -> Self {
        Self {
            node_id: node_id.into(),
            draft: false,
            auto_merge: None,
            can_enable_auto_merge: false,
            can_disable_auto_merge: false,
            settings,
        }
    }

    /// Whether it is a draft.
    #[must_use]
    pub fn with_draft(mut self, draft: bool) -> Self {
        self.draft = draft;
        self
    }

    /// The auto-merge set on it.
    #[must_use]
    pub fn with_auto_merge(mut self, auto_merge: EnabledAutoMerge) -> Self {
        self.auto_merge = Some(auto_merge);
        self
    }

    /// Whether the token may set auto-merge on it now.
    #[must_use]
    pub fn with_can_enable_auto_merge(mut self, can: bool) -> Self {
        self.can_enable_auto_merge = can;
        self
    }

    /// Whether the token may take its auto-merge off now.
    #[must_use]
    pub fn with_can_disable_auto_merge(mut self, can: bool) -> Self {
        self.can_disable_auto_merge = can;
        self
    }
}
