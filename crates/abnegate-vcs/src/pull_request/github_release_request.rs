use crate::pull_request::ReleaseRequest;
use crate::tag_name::TagName;
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
    #[serde(skip_serializing_if = "Option::is_none")]
    make_latest: Option<&'static str>,
}

impl<'a> GitHubReleaseRequest<'a> {
    /// The release `request` describes, published under `tag`, the tag it
    /// names once held to git's rules.
    pub(super) fn under(tag: &'a TagName, request: &'a ReleaseRequest) -> Self {
        Self {
            tag_name: tag.as_str(),
            target_commitish: request.target.as_deref(),
            name: request.name.as_deref(),
            body: request.body.as_deref(),
            draft: request.draft,
            prerelease: request.prerelease,
            make_latest: request.make_latest.map(|choice| choice.as_str()),
        }
    }
}
