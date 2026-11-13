use crate::pull_request::github_tagger::GitHubTagger;
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
    /// The tag `tag` on the commit `object`, carrying `message`, signed by
    /// `tagger`.
    pub(super) fn on_commit(
        tag: &'a str,
        message: &'a str,
        object: &'a str,
        tagger: GitHubTagger<'a>,
    ) -> Self {
        Self {
            tag,
            message,
            object,
            kind: COMMIT,
            tagger,
        }
    }
}
