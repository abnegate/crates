use serde::Deserialize;

/// One check run in a check suite, as GitHub's GraphQL API answers for it,
/// its status and conclusion in GitHub's capitals.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlCheckRun {
    #[serde(default)]
    pub(super) database_id: Option<u64>,
    #[serde(default)]
    pub(super) name: Option<String>,
    pub(super) status: String,
    #[serde(default)]
    pub(super) conclusion: Option<String>,
    #[serde(default)]
    pub(super) url: Option<String>,
    #[serde(default)]
    pub(super) details_url: Option<String>,
    #[serde(default)]
    pub(super) started_at: Option<String>,
    #[serde(default)]
    pub(super) completed_at: Option<String>,
}
