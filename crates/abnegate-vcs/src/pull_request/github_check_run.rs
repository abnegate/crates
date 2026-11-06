use crate::pull_request::github_check_conclusion::GitHubCheckConclusion;
use crate::pull_request::github_check_status::GitHubCheckStatus;
use serde::Deserialize;

/// The part of one check run on a commit its outcome is read from, and the
/// identifier that tells it from every other run when GitHub says.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubCheckRun {
    #[serde(default)]
    pub(super) id: Option<u64>,
    pub(super) name: String,
    pub(super) status: GitHubCheckStatus,
    #[serde(default)]
    pub(super) conclusion: Option<GitHubCheckConclusion>,
}
