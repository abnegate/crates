use crate::pull_request::graphql_workflow::GraphQlWorkflow;
use crate::pull_request::graphql_workflow_file::GraphQlWorkflowFile;
use serde::Deserialize;
use std::num::NonZeroU64;

/// The GitHub Actions run behind a check suite, as GitHub's GraphQL API
/// answers for it, without the status and conclusion its suite carries.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlWorkflowRun {
    pub(super) database_id: NonZeroU64,
    pub(super) event: String,
    pub(super) url: String,
    pub(super) created_at: String,
    pub(super) updated_at: String,
    pub(super) workflow: GraphQlWorkflow,
    #[serde(default)]
    pub(super) file: Option<GraphQlWorkflowFile>,
}
