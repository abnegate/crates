use crate::pull_request::graphql_app::GraphQlApp;
use crate::pull_request::graphql_branch::GraphQlBranch;
use crate::pull_request::graphql_check_run::GraphQlCheckRun;
use crate::pull_request::graphql_connection::GraphQlConnection;
use crate::pull_request::graphql_node::GraphQlNode;
use crate::pull_request::graphql_workflow_run::GraphQlWorkflowRun;
use serde::Deserialize;

/// One check suite on a commit, as GitHub's GraphQL API answers for it: the
/// repository it ran in, the App it belongs to, the Actions run behind it if any, and a page of the
/// latest attempt of each of its check runs.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlCheckSuite {
    pub(super) status: String,
    #[serde(default)]
    pub(super) conclusion: Option<String>,
    pub(super) created_at: String,
    #[serde(default)]
    pub(super) branch: Option<GraphQlBranch>,
    pub(super) repository: GraphQlNode,
    #[serde(default)]
    pub(super) app: Option<GraphQlApp>,
    #[serde(default)]
    pub(super) workflow_run: Option<GraphQlWorkflowRun>,
    #[serde(default)]
    pub(super) check_runs: Option<GraphQlConnection<GraphQlCheckRun>>,
}
