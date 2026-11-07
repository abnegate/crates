use serde::Deserialize;

/// A file as GitHub's contents API describes it. Only its base64 content is
/// read, and that may be absent.
#[derive(Debug, Deserialize)]
pub(super) struct GitHubContents {
    #[serde(default)]
    pub(super) content: Option<String>,
}
