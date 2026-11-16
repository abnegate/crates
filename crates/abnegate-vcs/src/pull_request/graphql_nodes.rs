use serde::Deserialize;
use serde_json::Value;

/// What GitHub's GraphQL API answers for a list of node ids: one entry per
/// id, in the order asked, each left unread so that one that cannot be read
/// fails only itself.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GraphQlNodes {
    pub(super) nodes: Vec<Value>,
}
