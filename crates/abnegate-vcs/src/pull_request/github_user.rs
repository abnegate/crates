use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubUser {
    #[serde(default)]
    pub(super) login: Option<String>,
}
