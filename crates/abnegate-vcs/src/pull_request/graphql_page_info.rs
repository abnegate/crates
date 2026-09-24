use serde::Deserialize;

/// Where one page of a GraphQL connection ended, and whether another follows.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlPageInfo {
    pub(super) has_next_page: bool,
    #[serde(default)]
    pub(super) end_cursor: Option<String>,
}
