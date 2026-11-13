use crate::pull_request::ReleaseRequest;
use serde::Serialize;

/// The body of a request to create a release; what the caller left unset is
/// not sent, so GitHub's own defaults apply to it.
#[derive(Debug, Clone, Serialize)]
pub(super) struct GitHubReleaseRequest<'a> {
    tag_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_commitish: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<&'a str>,
    draft: bool,
    prerelease: bool,
}

impl<'a> From<&'a ReleaseRequest> for GitHubReleaseRequest<'a> {
    fn from(request: &'a ReleaseRequest) -> Self {
        Self {
            tag_name: &request.tag,
            target_commitish: request.target.as_deref(),
            name: request.name.as_deref(),
            body: request.body.as_deref(),
            draft: request.draft,
            prerelease: request.prerelease,
        }
    }
}
