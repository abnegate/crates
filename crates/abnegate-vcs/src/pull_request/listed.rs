use crate::branch_name::BranchName;
use crate::commit_sha::CommitSha;
use crate::pull_request::PullRequestReference;
use crate::pull_request::PullRequestState;

/// A pull request as a page of a repository's pull requests lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ListedPullRequest {
    /// Where it lives: its repository and number.
    pub reference: PullRequestReference,
    /// GitHub's global node identifier, which its GraphQL API addresses the
    /// pull request by.
    pub node_id: String,
    /// Its title, empty when GitHub gives none.
    pub title: String,
    /// Where it can be read in a browser.
    pub url: String,
    /// Whether it is open.
    pub state: PullRequestState,
    /// Whether it is a draft.
    pub draft: bool,
    /// The branch it merges from.
    pub head: BranchName,
    /// The branch it merges into.
    pub base: BranchName,
    /// The commit at the tip of the branch it merges from, or `None` when
    /// GitHub does not say.
    pub head_sha: Option<CommitSha>,
    /// When it last changed, as GitHub's ISO 8601 timestamp, or `None` when
    /// GitHub does not say.
    pub updated_at: Option<String>,
}

impl ListedPullRequest {
    /// The pull request `reference` names, which GitHub's GraphQL API
    /// addresses as `node_id`, merging `head` into `base`, and readable at
    /// `url`.
    ///
    /// It is open, untitled, no draft, and its head commit and last change
    /// are unknown, until the `with_*` methods say otherwise.
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
            title: String::new(),
            url: url.into(),
            state: PullRequestState::Open,
            draft: false,
            head,
            base,
            head_sha: None,
            updated_at: None,
        }
    }

    /// Its title.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
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

    /// The commit at the tip of the branch it merges from.
    #[must_use]
    pub fn with_head_sha(mut self, head_sha: CommitSha) -> Self {
        self.head_sha = Some(head_sha);
        self
    }

    /// When it last changed, as GitHub's ISO 8601 timestamp.
    #[must_use]
    pub fn with_updated_at(mut self, updated_at: impl Into<String>) -> Self {
        self.updated_at = Some(updated_at.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::Repository;
    use std::num::NonZeroU64;

    fn commit() -> CommitSha {
        CommitSha::parse(&"a".repeat(40)).unwrap()
    }

    fn listed() -> ListedPullRequest {
        ListedPullRequest::new(
            PullRequestReference::new(
                Repository::new("acme", "project").unwrap(),
                NonZeroU64::new(7).unwrap(),
            ),
            "PR_kwDOAcme7",
            "https://github.com/acme/project/pull/7",
            BranchName::parse("task/one").unwrap(),
            BranchName::parse("main").unwrap(),
        )
    }

    #[test]
    fn a_listed_pull_request_built_from_what_names_it_is_open_and_untitled() {
        let built = listed();

        assert_eq!(built.node_id, "PR_kwDOAcme7");
        assert_eq!(built.url, "https://github.com/acme/project/pull/7");
        assert_eq!(built.head.as_str(), "task/one");
        assert_eq!(built.base.as_str(), "main");
        assert_eq!(built.title, "");
        assert_eq!(built.state, PullRequestState::Open);
        assert!(!built.draft);
        assert_eq!(built.head_sha, None);
        assert_eq!(built.updated_at, None);
    }

    #[test]
    fn each_builder_sets_its_own_field_and_no_other() {
        let built = listed()
            .with_title("(feat): basket totals")
            .with_state(PullRequestState::Closed)
            .with_draft(true)
            .with_head_sha(commit())
            .with_updated_at("2026-11-06T09:30:00Z");

        assert_eq!(
            built,
            ListedPullRequest {
                title: "(feat): basket totals".to_string(),
                state: PullRequestState::Closed,
                draft: true,
                head_sha: Some(commit()),
                updated_at: Some("2026-11-06T09:30:00Z".to_string()),
                ..listed()
            }
        );
    }
}
