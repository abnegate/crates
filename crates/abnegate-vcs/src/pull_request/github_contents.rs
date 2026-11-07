use serde::Deserialize;

/// A file as GitHub's contents API describes it. Only its base64 content and
/// its size are read, and either may be absent.
#[derive(Debug, Deserialize)]
pub(super) struct GitHubContents {
    pub(super) content: Option<String>,
    pub(super) size: Option<u64>,
}
