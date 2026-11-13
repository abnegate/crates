use crate::pull_request::Tagger;
use serde::Serialize;

/// Who signs an annotated tag, as a request to create one sends it.
#[derive(Debug, Clone, Serialize)]
pub(super) struct GitHubTagger<'a> {
    name: &'a str,
    email: &'a str,
}

impl<'a> From<&'a Tagger> for GitHubTagger<'a> {
    fn from(tagger: &'a Tagger) -> Self {
        Self {
            name: &tagger.name,
            email: &tagger.email,
        }
    }
}
