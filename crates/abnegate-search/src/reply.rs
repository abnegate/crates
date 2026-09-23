//! The part of a SearXNG JSON answer this crate reads.

use serde::Deserialize;

use crate::entry::Entry;

#[derive(Debug, Deserialize)]
pub(crate) struct Reply {
    #[serde(default)]
    pub(crate) results: Vec<Entry>,
}
