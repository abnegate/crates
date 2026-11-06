use serde::Deserialize;

/// Where one page of a GraphQL connection ended, and whether another follows,
/// if GitHub said.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlPageInfo {
    #[serde(default)]
    pub(super) has_next_page: Option<bool>,
    #[serde(default)]
    pub(super) end_cursor: Option<String>,
}
