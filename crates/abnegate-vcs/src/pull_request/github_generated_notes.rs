use crate::pull_request::GeneratedNotes;
use serde::Deserialize;

/// Release notes as the GitHub REST API writes them; both parts are required.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubGeneratedNotes {
    name: String,
    body: String,
}

impl From<GitHubGeneratedNotes> for GeneratedNotes {
    fn from(notes: GitHubGeneratedNotes) -> Self {
        Self::new(notes.name, notes.body)
    }
}
