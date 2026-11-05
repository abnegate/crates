use crate::pull_request::Release;
use serde::Deserialize;

/// A release as the GitHub REST API returns it.
///
/// Only its identifier and tag are required: GitHub sends the rest on every
/// release, but an answer that leaves any of it out still names the release.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubRelease {
    id: u64,
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: Option<bool>,
    #[serde(default)]
    prerelease: Option<bool>,
}

impl From<GitHubRelease> for Release {
    fn from(release: GitHubRelease) -> Self {
        Self {
            id: release.id,
            tag: release.tag_name,
            name: release.name,
            url: release.html_url.unwrap_or_default(),
            published_at: release.published_at,
            draft: release.draft.unwrap_or_default(),
            prerelease: release.prerelease.unwrap_or_default(),
        }
    }
}
