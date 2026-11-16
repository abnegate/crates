use serde::Deserialize;

/// Whether one review thread is resolved, as GitHub's GraphQL API answers for
/// it, without its id or comments.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlThreadResolution {
    pub(super) is_resolved: bool,
}
