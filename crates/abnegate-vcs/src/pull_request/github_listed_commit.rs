use crate::commit_sha::CommitSha;
use crate::pull_request::Commit;
use crate::pull_request::PullRequestResult;
use crate::pull_request::github_commit_detail::GitHubCommitDetail;
use crate::pull_request::github_user::GitHubUser;
use serde::Deserialize;

/// A commit as the GitHub REST API lists a pull request's commits.
///
/// Its identifier, link and message are required, as GitHub sends them on
/// every commit, so an answer that leaves any of them out is unreadable. The
/// GitHub account behind it is `null` when no account claims its author's
/// email.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubListedCommit {
    sha: String,
    html_url: String,
    commit: GitHubCommitDetail,
    #[serde(default)]
    author: Option<GitHubUser>,
}

impl GitHubListedCommit {
    /// The commit this row describes; an identifier this crate does not
    /// accept is [`crate::pull_request::PullRequestError::Parse`].
    pub(super) fn commit(self) -> PullRequestResult<Commit> {
        let signature = self.commit.author;
        Ok(Commit {
            sha: CommitSha::parse(&self.sha)?,
            message: self.commit.message,
            author_name: signature.as_ref().and_then(|author| author.name.clone()),
            author_login: self.author.and_then(|account| account.login),
            authored_at: signature.and_then(|author| author.date),
            url: self.html_url,
        })
    }
}
