use crate::pull_request::ListedPullRequest;
use crate::pull_request::RepositoryId;

/// One page of a repository's open pull requests, the most recently updated
/// first, as
/// [`PullRequestService::list_open_pull_requests`](crate::pull_request::PullRequestService::list_open_pull_requests)
/// reads it, and the cursor of the page after it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OpenPullRequestPage {
    /// The repository the pull requests are in, which
    /// [`PullRequestService::fetch_pull_request_evidence`](crate::pull_request::PullRequestService::fetch_pull_request_evidence)
    /// holds what it reads to, so that a fork's checks are told from the
    /// repository's own.
    pub repository_id: RepositoryId,
    /// The open pull requests on the page, the most recently updated first.
    pub items: Vec<ListedPullRequest>,
    /// The cursor the next page is read after, or `None` when GitHub said
    /// this page was the last.
    pub next: Option<String>,
}

impl OpenPullRequestPage {
    /// `items` in the repository `repository_id` names, followed by the page
    /// after the cursor `next` when there is one.
    pub fn new(
        repository_id: RepositoryId,
        items: Vec<ListedPullRequest>,
        next: Option<String>,
    ) -> Self {
        Self {
            repository_id,
            items,
            next,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_holds_what_it_was_made_from() {
        let page = OpenPullRequestPage::new(
            RepositoryId::new("R_kgDOAcme"),
            Vec::new(),
            Some("Y3Vyc29y".to_string()),
        );

        assert_eq!(page.repository_id, RepositoryId::new("R_kgDOAcme"));
        assert!(page.items.is_empty());
        assert_eq!(page.next.as_deref(), Some("Y3Vyc29y"));
    }
}
