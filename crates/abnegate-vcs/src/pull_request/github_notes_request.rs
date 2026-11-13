use serde::Serialize;

/// The body of a request for GitHub to write release notes; what the caller
/// left unset is not sent, so GitHub's own defaults apply to it.
#[derive(Debug, Clone, Serialize)]
pub(super) struct GitHubNotesRequest<'a> {
    pub(super) tag_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) target_commitish: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) previous_tag_name: Option<&'a str>,
}
