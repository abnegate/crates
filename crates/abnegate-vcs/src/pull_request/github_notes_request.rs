use serde::Serialize;

/// The body of a request for GitHub to write release notes.
#[derive(Debug, Clone, Serialize)]
pub(super) struct GitHubNotesRequest<'a> {
    pub(super) tag_name: &'a str,
    pub(super) target_commitish: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) previous_tag_name: Option<&'a str>,
}
