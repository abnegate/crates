use crate::pull_request::github_user::GitHubUser;
use serde::Deserialize;

/// A repository as GitHub describes it on creating it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubCreatedRepository {
    pub(super) name: String,
    pub(super) owner: GitHubUser,
    pub(super) html_url: String,
    pub(super) clone_url: String,
    pub(super) default_branch: String,
}
