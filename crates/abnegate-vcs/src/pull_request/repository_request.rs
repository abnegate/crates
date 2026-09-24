use serde::Serialize;

/// The body of a request to create a repository.
#[derive(Debug, Clone, Serialize)]
pub(super) struct RepositoryRequest<'a> {
    pub(super) name: &'a str,
    pub(super) description: &'a str,
    pub(super) private: bool,
    pub(super) auto_init: bool,
}
