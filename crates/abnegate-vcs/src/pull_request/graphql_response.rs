use crate::pull_request::graphql_error::GraphQlError;
use serde::Deserialize;

/// What GitHub's GraphQL API answers: the data asked for, the errors that
/// kept some or all of it back, or both.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GraphQlResponse {
    #[serde(default)]
    pub(super) data: Option<serde_json::Value>,
    #[serde(default)]
    pub(super) errors: Vec<GraphQlError>,
}
