use serde::Deserialize;
use std::num::NonZeroU64;

/// The workflow a run is of, as GitHub's GraphQL API answers for it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlWorkflow {
    pub(super) database_id: NonZeroU64,
    pub(super) name: String,
}
