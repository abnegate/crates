use crate::object_id::ObjectId;
use crate::pull_request::GitReference;
use crate::pull_request::PullRequestResult;
use crate::pull_request::github_reference_object::GitHubReferenceObject;
use serde::Deserialize;

/// A git reference as the GitHub REST API returns it, read only for the
/// object it points at.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubReference {
    object: GitHubReferenceObject,
}

impl GitHubReference {
    /// The reference this answer describes; an object named by anything but
    /// a full object name is [`crate::pull_request::PullRequestError::Parse`].
    pub(super) fn reference(self) -> PullRequestResult<GitReference> {
        Ok(GitReference::new(
            ObjectId::parse(&self.object.sha)?,
            self.object.kind,
        ))
    }
}
