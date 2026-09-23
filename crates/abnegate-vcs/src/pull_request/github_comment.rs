use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubComment {
    #[serde(default)]
    pub(super) body: Option<String>,
}
