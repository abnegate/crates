use serde::Deserialize;

/// A repository GitHub names inside another record, read only for its
/// `owner/name`.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubRepositoryName {
    pub(super) full_name: String,
}
