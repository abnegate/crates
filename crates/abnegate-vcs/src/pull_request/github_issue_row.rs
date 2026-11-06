use crate::pull_request::github_issue::GitHubIssue;
use crate::pull_request::github_issue_pull_request::GitHubIssuePullRequest;
use serde::Deserialize;

/// A row GitHub's REST API lists among a repository's issues: a pull request,
/// known by the object GitHub sets on it and read no further, or else an
/// issue, read whole.
///
/// A pull request is tried first, so nothing GitHub sends of one is required
/// before it is known to be one.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum GitHubIssueRow {
    PullRequest {
        #[expect(
            dead_code,
            reason = "its presence alone tells a pull request's row from an issue's"
        )]
        pull_request: GitHubIssuePullRequest,
    },
    Issue(Box<GitHubIssue>),
}
