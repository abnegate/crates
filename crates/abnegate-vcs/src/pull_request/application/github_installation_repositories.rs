use crate::pull_request::application::github_installed_repository::GitHubInstalledRepository;
use serde::Deserialize;

/// A page of the repositories an installation can reach, as GitHub answers
/// it. Only the rows are read: the `total_count` beside them counts every
/// repository the installation reaches, so it says nothing about this page.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubInstallationRepositories {
    pub(super) repositories: Vec<GitHubInstalledRepository>,
}
