use serde::Deserialize;
use serde::Serialize;

/// A page of an evidence record or of the evidence catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Evidence {
    /// The record the page is from.
    pub id: String,
    /// The page itself.
    pub content: String,
    /// Where in the record the page starts.
    pub offset: u64,
    /// Where the next page starts, when there is one.
    pub next: Option<u64>,
    /// The length of the whole record.
    pub total: u64,
}

impl Evidence {
    /// The last page of record `id`: `content` from `offset` of a record
    /// `total` long.
    ///
    /// The arguments come in the fields' order: the record's id before the
    /// page, and where the page starts before the record's length. Each pair
    /// shares a type, so the compiler cannot catch either one swapped.
    pub fn new(id: impl Into<String>, content: impl Into<String>, offset: u64, total: u64) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            offset,
            next: None,
            total,
        }
    }

    /// The same page, with more of the record from `next`.
    pub fn with_next(mut self, next: u64) -> Self {
        self.next = Some(next);
        self
    }
}
