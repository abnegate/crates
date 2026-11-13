use serde::Serialize;

/// The body of a request that changes whether a release is a draft.
#[derive(Debug, Clone, Serialize)]
pub(super) struct GitHubReleaseUpdate {
    pub(super) draft: bool,
}
