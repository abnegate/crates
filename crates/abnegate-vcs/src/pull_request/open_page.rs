use crate::pull_request::ListedPullRequest;

/// One page of a repository's open pull requests, the most recently updated
/// first, as
/// [`PullRequestService::fetch_open_pull_requests`](crate::pull_request::PullRequestService::fetch_open_pull_requests)
/// reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OpenPage {
    /// GitHub's node identifier for the repository, which stays the same
    /// when the repository is renamed or transferred.
    /// [`PullRequestService::fetch_pull_request_evidence`](crate::pull_request::PullRequestService::fetch_pull_request_evidence)
    /// takes it to tell the repository's own checks from a fork's.
    pub repository_id: String,
    /// The open pull requests on the page, the most recently updated first.
    pub pull_requests: Vec<ListedPullRequest>,
    /// The cursor the next page is read after, or `None` when GitHub said
    /// this page was the last.
    pub after: Option<String>,
}

impl OpenPage {
    /// `pull_requests` in the repository GitHub identifies by
    /// `repository_id`, followed by the page after the cursor `after` when
    /// there is one.
    pub fn new(
        repository_id: impl Into<String>,
        pull_requests: Vec<ListedPullRequest>,
        after: Option<String>,
    ) -> Self {
        Self {
            repository_id: repository_id.into(),
            pull_requests,
            after,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_holds_what_it_was_made_from() {
        let page = OpenPage::new("R_kgDOAcme", Vec::new(), Some("Y3Vyc29y".to_string()));

        assert_eq!(page.repository_id, "R_kgDOAcme");
        assert!(page.pull_requests.is_empty());
        assert_eq!(page.after.as_deref(), Some("Y3Vyc29y"));
    }
}
