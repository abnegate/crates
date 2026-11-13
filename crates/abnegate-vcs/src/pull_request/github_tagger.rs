use serde::Serialize;

/// Who signs an annotated tag, as a request to create one sends it.
#[derive(Debug, Clone, Serialize)]
pub(super) struct GitHubTagger<'a> {
    pub(super) name: &'a str,
    pub(super) email: &'a str,
}
