use crate::commit_sha::CommitSha;
use crate::pull_request::PullRequestResult;
use crate::pull_request::Tag;
use crate::pull_request::github_commit::GitHubCommit;
use serde::Deserialize;

/// A tag as the GitHub REST API lists a repository's tags.
///
/// Its name and commit are required, as GitHub sends them on every tag, so an
/// answer that leaves either out is unreadable rather than a tag with
/// something made up in its place.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubTag {
    name: String,
    commit: GitHubCommit,
}

impl GitHubTag {
    /// The tag this row describes; a commit this crate does not accept is
    /// [`crate::pull_request::PullRequestError::Parse`].
    pub(super) fn tag(self) -> PullRequestResult<Tag> {
        Ok(Tag::new(self.name, CommitSha::parse(&self.commit.sha)?))
    }
}
