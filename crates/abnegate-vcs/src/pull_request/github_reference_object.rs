use crate::pull_request::ObjectKind;
use serde::Deserialize;

/// The object a git reference points at, as the GitHub REST API describes
/// it; its name is required, and a kind it leaves out is unknown.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubReferenceObject {
    pub(super) sha: String,
    #[serde(rename = "type", default)]
    pub(super) kind: ObjectKind,
}
