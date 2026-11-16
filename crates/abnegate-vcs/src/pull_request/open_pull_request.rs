use crate::pull_request::Collected;
use crate::pull_request::HeadChecks;
use crate::pull_request::IssueComment;
use crate::pull_request::ListedPullRequest;
use crate::pull_request::MergeableState;
use crate::pull_request::ReviewThreadCount;

/// An open pull request with everything that says whether it is ready to
/// merge, as
/// [`PullRequestService::fetch_pull_request_evidence`](crate::pull_request::PullRequestService::fetch_pull_request_evidence)
/// reads it in one request alongside others.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OpenPullRequest {
    /// The pull request as a listing names it, read in the same request as
    /// everything else here.
    pub listed: ListedPullRequest,
    /// What stands between it and a merge, as GitHub sums it up: the same
    /// word its REST API gives, a draft's included.
    pub mergeable_state: MergeableState,
    /// How many of its first hundred review threads are open and resolved,
    /// complete only when it has no more than those.
    pub threads: ReviewThreadCount,
    /// Its newest hundred conversation comments, oldest first, complete only
    /// when it has no more than those.
    pub comments: Collected<IssueComment>,
    /// What checked its head commit, or `None` when the newest commit GitHub
    /// listed for it is not the head it named, as while a push lands.
    pub head: Option<HeadChecks>,
}

impl OpenPullRequest {
    /// `listed`, standing at `mergeable_state`, with `threads` and
    /// `comments`, and nothing read of its head until [`Self::with_head`]
    /// says otherwise.
    pub fn new(
        listed: ListedPullRequest,
        mergeable_state: MergeableState,
        threads: ReviewThreadCount,
        comments: Collected<IssueComment>,
    ) -> Self {
        Self {
            listed,
            mergeable_state,
            threads,
            comments,
            head: None,
        }
    }

    /// What checked its head commit.
    #[must_use]
    pub fn with_head(mut self, head: HeadChecks) -> Self {
        self.head = Some(head);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::branch_name::BranchName;
    use crate::commit_sha::CommitSha;
    use crate::pull_request::PullRequestReference;
    use crate::pull_request::Repository;
    use std::num::NonZeroU64;

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
    fn an_open_pull_request_reads_no_head_until_given_one() {
        let bare = OpenPullRequest::new(
            listed(),
            MergeableState::Blocked,
            ReviewThreadCount::new(1, 2, true),
            Collected::new(Vec::new(), true),
        );
        let head = HeadChecks::new(
            CommitSha::parse(&"a".repeat(40)).unwrap(),
            Collected::new(Vec::new(), true),
            Collected::new(Vec::new(), true),
            Collected::new(Vec::new(), true),
        );

        assert_eq!(bare.head, None);
        assert_eq!(bare.mergeable_state, MergeableState::Blocked);
        assert_eq!(bare.threads, ReviewThreadCount::new(1, 2, true));
        assert_eq!(
            bare.clone().with_head(head.clone()),
            OpenPullRequest {
                head: Some(head),
                ..bare
            }
        );
    }
}
