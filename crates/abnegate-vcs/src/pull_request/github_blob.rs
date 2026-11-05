use serde::Deserialize;

/// A git blob as GitHub's REST API returns it: its content in the encoding
/// named, and its size in bytes once decoded.
#[derive(Debug, Deserialize)]
pub(super) struct GitHubBlob {
    pub(super) sha: String,
    pub(super) size: u64,
    pub(super) content: String,
    pub(super) encoding: String,
}
