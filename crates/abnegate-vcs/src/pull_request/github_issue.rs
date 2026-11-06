use crate::pull_request::Issue;
use crate::pull_request::IssueReference;
use crate::pull_request::IssueState;
use crate::pull_request::Label;
use crate::pull_request::Repository;
use crate::pull_request::github_issue_pull_request::GitHubIssuePullRequest;
use crate::pull_request::github_label::GitHubLabel;
use crate::pull_request::github_user::GitHubUser;
use serde::Deserialize;
use std::num::NonZeroU64;

/// An issue, or a pull request read as one, as the GitHub REST API returns it.
///
/// What GitHub sends on every issue is required, so an answer that leaves any
/// of it out is unreadable rather than an issue with something made up in its
/// place. What GitHub may leave out or send as `null`, an issue's body, author,
/// assignees and close time, is read as not said.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubIssue {
    number: NonZeroU64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    state: IssueState,
    html_url: String,
    #[serde(default)]
    user: Option<GitHubUser>,
    labels: Vec<GitHubLabel>,
    #[serde(default)]
    assignees: Option<Vec<GitHubUser>>,
    comments: u64,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    closed_at: Option<String>,
    #[serde(default)]
    pull_request: Option<GitHubIssuePullRequest>,
}

impl GitHubIssue {
    /// The issue this row describes, in `repository`.
    pub(super) fn within(self, repository: &Repository) -> Issue {
        Issue {
            reference: IssueReference::new(repository.clone(), self.number),
            title: self.title,
            body: self.body,
            state: self.state,
            url: self.html_url,
            author: self.user.and_then(|user| user.login),
            labels: self.labels.into_iter().map(Label::from).collect(),
            assignees: self.assignees.map(|assignees| {
                assignees
                    .into_iter()
                    .filter_map(|assignee| assignee.login)
                    .collect()
            }),
            comments: self.comments,
            created_at: self.created_at,
            updated_at: self.updated_at,
            closed_at: self.closed_at,
            pull_request: self.pull_request.is_some(),
        }
    }
}
