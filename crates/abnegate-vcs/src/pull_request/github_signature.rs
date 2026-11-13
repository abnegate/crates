use serde::Deserialize;

/// Who git records as a commit's author or committer, and when, as the
/// GitHub REST API returns it; either may be left out.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubSignature {
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) date: Option<String>,
}
