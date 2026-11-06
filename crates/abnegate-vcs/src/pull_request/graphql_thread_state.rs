use serde::Deserialize;

/// Whether one review thread is resolved, as GitHub's GraphQL API answers for
/// it, without its comments.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlThreadState {
    pub(super) id: String,
    pub(super) is_resolved: bool,
}
