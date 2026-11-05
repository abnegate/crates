use crate::commit_sha::CommitSha;
use crate::parse_error::ParseError;
use crate::pull_request::CheckRun;
use serde::Deserialize;

/// One check run on a commit as the GitHub REST API returns it, its status
/// and conclusion kept as GitHub's own words. Only the status is required.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubCheckRunRecord {
    #[serde(default)]
    pub(super) id: Option<u64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    head_sha: Option<String>,
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    details_url: Option<String>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    completed_at: Option<String>,
}

impl TryFrom<GitHubCheckRunRecord> for CheckRun {
    type Error = ParseError;

    fn try_from(run: GitHubCheckRunRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: run.id,
            name: run.name,
            head_sha: run.head_sha.as_deref().map(CommitSha::parse).transpose()?,
            status: run.status,
            conclusion: run.conclusion,
            url: run.html_url,
            details_url: run.details_url,
            started_at: run.started_at,
            completed_at: run.completed_at,
        })
    }
}
