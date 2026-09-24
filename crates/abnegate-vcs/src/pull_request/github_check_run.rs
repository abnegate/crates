use crate::pull_request::github_check_conclusion::GitHubCheckConclusion;
use crate::pull_request::github_check_status::GitHubCheckStatus;
use serde::Deserialize;

/// The part of one check run on a commit its outcome is read from.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubCheckRun {
    pub(super) name: String,
    pub(super) status: GitHubCheckStatus,
    #[serde(default)]
    pub(super) conclusion: Option<GitHubCheckConclusion>,
}
