use crate::branch_name::BranchName;
use crate::pull_request::PullRequestReference;
use crate::pull_request::PullRequestState;

/// A pull request found by the branch it merges from and the branch it merges
/// into.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FoundPullRequest {
    /// Where it lives: its repository and number.
    pub reference: PullRequestReference,
    /// GitHub's global node identifier, which its GraphQL API addresses the
    /// pull request by.
    pub node_id: String,
    /// Where it can be read in a browser.
    pub url: String,
    /// The branch it merges from.
    pub head: BranchName,
    /// The branch it merges into.
    pub base: BranchName,
    /// Whether it is open.
    pub state: PullRequestState,
    /// Whether it is a draft.
    pub draft: bool,
    /// Whether it merged.
    pub merged: bool,
}

impl FoundPullRequest {
    /// The pull request `reference` names, which GitHub's GraphQL API
    /// addresses as `node_id`, merging `head` into `base`, and readable at
    /// `url`.
    ///
    /// It is open, no draft, and has not merged, until the `with_*` methods
    /// say otherwise.
    pub fn new(
        reference: PullRequestReference,
        node_id: impl Into<String>,
        url: impl Into<String>,
        head: BranchName,
        base: BranchName,
    ) -> Self {
        Self {
            reference,
            node_id: node_id.into(),
            url: url.into(),
            head,
            base,
            state: PullRequestState::Open,
            draft: false,
            merged: false,
        }
    }

    /// Whether it is open.
    #[must_use]
    pub fn with_state(mut self, state: PullRequestState) -> Self {
        self.state = state;
        self
    }

    /// Whether it is a draft.
    #[must_use]
    pub fn with_draft(mut self, draft: bool) -> Self {
        self.draft = draft;
        self
    }

    /// Whether it merged.
    #[must_use]
    pub fn with_merged(mut self, merged: bool) -> Self {
        self.merged = merged;
        self
    }
}
