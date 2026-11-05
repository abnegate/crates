use crate::pull_request::Issue;
use crate::pull_request::IssueReference;
use crate::pull_request::IssueState;
use crate::pull_request::Label;
use crate::pull_request::Repository;
use crate::pull_request::github_label::GitHubLabel;
use crate::pull_request::github_user::GitHubUser;
use serde::Deserialize;
use serde::de::IgnoredAny;
use std::num::NonZeroU64;

/// An issue, or a pull request read as one, as the GitHub REST API returns it.
///
/// Only its number is required: GitHub sends the rest on every issue, but an
/// answer that leaves any of it out still names the issue.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubIssue {
    number: NonZeroU64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    state: Option<IssueState>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    user: Option<GitHubUser>,
    #[serde(default)]
    labels: Option<Vec<GitHubLabel>>,
    #[serde(default)]
    assignees: Option<Vec<GitHubUser>>,
    #[serde(default)]
    comments: Option<u64>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    closed_at: Option<String>,
    #[serde(default)]
    pull_request: Option<IgnoredAny>,
}

impl GitHubIssue {
    /// The issue this row describes, in `repository`.
    pub(super) fn within(self, repository: &Repository) -> Issue {
        Issue {
            reference: IssueReference::new(repository.clone(), self.number),
            title: self.title.unwrap_or_default(),
            body: self.body,
            state: self.state.unwrap_or(IssueState::Unknown),
            url: self.html_url.unwrap_or_default(),
            author: self.user.and_then(|user| user.login).unwrap_or_default(),
            labels: self
                .labels
                .unwrap_or_default()
                .into_iter()
                .map(Label::from)
                .collect(),
            assignees: self
                .assignees
                .unwrap_or_default()
                .into_iter()
                .filter_map(|assignee| assignee.login)
                .collect(),
            comments: self.comments.unwrap_or_default(),
            created_at: self.created_at.unwrap_or_default(),
            updated_at: self.updated_at,
            closed_at: self.closed_at,
            pull_request: self.pull_request.is_some(),
        }
    }
}
