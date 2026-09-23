//! One result as SearXNG sends it.

use serde::Deserialize;

use crate::hit::SearchHit;

#[derive(Debug, Deserialize)]
pub(crate) struct Entry {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

impl Entry {
    /// The hit this entry describes, or nothing when it has no URL to cite.
    pub(crate) fn into_hit(self) -> Option<SearchHit> {
        let url = self.url.filter(|url| !url.is_empty())?;
        let title = self
            .title
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| url.clone());
        Some(SearchHit {
            title,
            url,
            snippet: self.content.unwrap_or_default(),
            identifier: None,
        })
    }
}
