use crate::pull_request::Release;
use serde::Deserialize;

/// A release as the GitHub REST API returns it.
///
/// What GitHub sends on every release is required, so an answer that leaves
/// any of it out is unreadable rather than a release with something made up
/// in its place. Its title, publication time, notes and target may be `null`,
/// and are read as not said.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubRelease {
    id: u64,
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    html_url: String,
    #[serde(default)]
    published_at: Option<String>,
    draft: bool,
    prerelease: bool,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    target_commitish: Option<String>,
}

impl From<GitHubRelease> for Release {
    fn from(release: GitHubRelease) -> Self {
        Self {
            id: release.id,
            tag: release.tag_name,
            name: release.name,
            url: release.html_url,
            published_at: release.published_at,
            draft: release.draft,
            prerelease: release.prerelease,
            body: release.body,
            target: release.target_commitish,
        }
    }
}
