use serde::Serialize;

/// The body of a request to create a git reference pointing at an object.
#[derive(Debug, Clone, Serialize)]
pub(super) struct GitHubReferenceRequest<'a> {
    #[serde(rename = "ref")]
    pub(super) reference: &'a str,
    pub(super) sha: &'a str,
}
