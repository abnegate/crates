use serde::Deserialize;

/// The part of a repository's details this service reads.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct RepositoryDetail {
    pub(super) default_branch: String,
}
