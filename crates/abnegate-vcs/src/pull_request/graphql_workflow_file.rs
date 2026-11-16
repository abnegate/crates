use serde::Deserialize;

/// The file a workflow run was defined by, as GitHub's GraphQL API answers
/// for it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GraphQlWorkflowFile {
    pub(super) path: String,
}
