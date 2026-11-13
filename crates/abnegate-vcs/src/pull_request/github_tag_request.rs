use crate::pull_request::TagRequest;
use crate::pull_request::github_tagger::GitHubTagger;
use crate::tag_name::TagName;
use serde::Serialize;

/// The kind of object an annotated tag this crate creates points at.
const COMMIT: &str = "commit";

/// The body of a request to create an annotated tag object.
#[derive(Debug, Clone, Serialize)]
pub(super) struct GitHubTagRequest<'a> {
    tag: &'a str,
    message: &'a str,
    object: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    tagger: GitHubTagger<'a>,
}

impl<'a> GitHubTagRequest<'a> {
    /// The tag `request` describes, named `tag`, the name it gives once held
    /// to git's rules, on its commit.
    pub(super) fn under(tag: &'a TagName, request: &'a TagRequest) -> Self {
        Self {
            tag: tag.as_str(),
            message: &request.message,
            object: request.commit.as_str(),
            kind: COMMIT,
            tagger: GitHubTagger::from(&request.tagger),
        }
    }
}
