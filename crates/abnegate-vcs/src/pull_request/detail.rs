use crate::branch_name::BranchName;
use crate::commit_sha::CommitSha;
use crate::pull_request::Mergeability;
use crate::pull_request::MergeableState;
use crate::pull_request::PullRequestState;
use std::num::NonZeroU64;

/// A pull request as GitHub describes it now.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PullRequestDetail {
    /// GitHub's global node identifier, which its GraphQL API addresses the
    /// pull request by.
    pub node_id: String,
    /// The number it goes by in its repository.
    pub number: NonZeroU64,
    /// Its title.
    pub title: String,
    /// Its body, if it has one.
    pub body: Option<String>,
    /// Whether it is open.
    pub state: PullRequestState,
    /// Whether it is a draft.
    pub draft: bool,
    /// Whether it merged.
    pub merged: bool,
    /// Once merged, the commit the merge made; while open, the test merge
    /// GitHub last prepared. `None` until GitHub has made either.
    pub merge_commit_sha: Option<CommitSha>,
    /// The branch it merges from.
    pub head: BranchName,
    /// The commit at the tip of that branch.
    pub head_sha: CommitSha,
    /// The branch it merges into.
    pub base: BranchName,
    /// Whether the head still merges with the base.
    pub mergeable: Mergeability,
    /// What stands between it and a merge, as GitHub sums it up.
    pub mergeable_state: MergeableState,
    /// Files it changes.
    pub changed_files: u32,
    /// Lines it adds.
    pub additions: u32,
    /// Lines it removes.
    pub deletions: u32,
    /// Commits on its branch.
    pub commits: u32,
    /// Where it can be read in a browser.
    pub url: String,
}

impl PullRequestDetail {
    /// The pull request numbered `number` in its repository, which GitHub's
    /// GraphQL API addresses as `node_id`, titled `title`, merging `head` at
    /// `head_sha` into `base`, and readable at `url`.
    ///
    /// It is open, has no body, is no draft, has not merged, has no merge
    /// commit, changes nothing, and GitHub has yet to work out whether it
    /// merges or what stands in its way, until the `with_*` methods say
    /// otherwise.
    pub fn new(
        node_id: impl Into<String>,
        number: NonZeroU64,
        title: impl Into<String>,
        head: BranchName,
        head_sha: CommitSha,
        base: BranchName,
        url: impl Into<String>,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            number,
            title: title.into(),
            body: None,
            state: PullRequestState::Open,
            draft: false,
            merged: false,
            merge_commit_sha: None,
            head,
            head_sha,
            base,
            mergeable: Mergeability::Unknown,
            mergeable_state: MergeableState::Unknown,
            changed_files: 0,
            additions: 0,
            deletions: 0,
            commits: 0,
            url: url.into(),
        }
    }

    /// Its body.
    #[must_use]
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
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

    /// Whether it merged.
    #[must_use]
    pub fn with_merged(mut self, merged: bool) -> Self {
        self.merged = merged;
        self
    }

    /// The commit its merge made, or the test merge GitHub last prepared
    /// while it is open.
    #[must_use]
    pub fn with_merge_commit_sha(mut self, sha: CommitSha) -> Self {
        self.merge_commit_sha = Some(sha);
        self
    }

    /// Whether the head still merges with the base.
    #[must_use]
    pub fn with_mergeable(mut self, mergeable: Mergeability) -> Self {
        self.mergeable = mergeable;
        self
    }

    /// What stands between it and a merge, as GitHub sums it up.
    #[must_use]
    pub fn with_mergeable_state(mut self, state: MergeableState) -> Self {
        self.mergeable_state = state;
        self
    }

    /// How many files it changes.
    #[must_use]
    pub fn with_changed_files(mut self, changed_files: u32) -> Self {
        self.changed_files = changed_files;
        self
    }

    /// How many lines it adds.
    #[must_use]
    pub fn with_additions(mut self, additions: u32) -> Self {
        self.additions = additions;
        self
    }

    /// How many lines it removes.
    #[must_use]
    pub fn with_deletions(mut self, deletions: u32) -> Self {
        self.deletions = deletions;
        self
    }

    /// How many commits are on its branch.
    #[must_use]
    pub fn with_commits(mut self, commits: u32) -> Self {
        self.commits = commits;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(digit: char) -> CommitSha {
        CommitSha::parse(&digit.to_string().repeat(40)).unwrap()
    }

    fn opened() -> PullRequestDetail {
        PullRequestDetail::new(
            "PR_kwDOAcme7",
            NonZeroU64::new(7).unwrap(),
            "(feat): basket totals",
            BranchName::parse("task/one").unwrap(),
            commit('a'),
            BranchName::parse("main").unwrap(),
            "https://github.com/acme/project/pull/7",
        )
    }

    #[test]
    fn a_pull_request_built_from_what_names_it_is_open_and_changes_nothing() {
        assert_eq!(
            opened(),
            PullRequestDetail {
                node_id: "PR_kwDOAcme7".to_string(),
                number: NonZeroU64::new(7).unwrap(),
                title: "(feat): basket totals".to_string(),
                body: None,
                state: PullRequestState::Open,
                draft: false,
                merged: false,
                merge_commit_sha: None,
                head: BranchName::parse("task/one").unwrap(),
                head_sha: commit('a'),
                base: BranchName::parse("main").unwrap(),
                mergeable: Mergeability::Unknown,
                mergeable_state: MergeableState::Unknown,
                changed_files: 0,
                additions: 0,
                deletions: 0,
                commits: 0,
                url: "https://github.com/acme/project/pull/7".to_string(),
            }
        );
    }

    #[test]
    fn each_builder_sets_its_own_field_and_no_other() {
        let built = opened()
            .with_body("Adds totals to the basket.")
            .with_state(PullRequestState::Closed)
            .with_draft(true)
            .with_merged(true)
            .with_merge_commit_sha(commit('c'))
            .with_mergeable(Mergeability::Conflicted)
            .with_mergeable_state(MergeableState::Dirty)
            .with_changed_files(3)
            .with_additions(120)
            .with_deletions(14)
            .with_commits(2);

        assert_eq!(
            built,
            PullRequestDetail {
                body: Some("Adds totals to the basket.".to_string()),
                state: PullRequestState::Closed,
                draft: true,
                merged: true,
                merge_commit_sha: Some(commit('c')),
                mergeable: Mergeability::Conflicted,
                mergeable_state: MergeableState::Dirty,
                changed_files: 3,
                additions: 120,
                deletions: 14,
                commits: 2,
                ..opened()
            }
        );
    }
}
