//! One hit from a single retrieval leg.

/// One hit from a single retrieval leg, in that leg's rank order.
///
/// `score` is the keyword leg's full-text rank or the semantic leg's cosine
/// similarity; fusion uses only the position, and keeps the score for
/// reporting.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Candidate<Id> {
    /// Identifies the chunk across both legs.
    pub id: Id,
    /// The chunk's text.
    pub chunk_text: String,
    /// The URI of the item the chunk belongs to.
    pub item_uri: String,
    /// The title of the item the chunk belongs to.
    pub item_title: String,
    /// The leg's own score for the chunk.
    pub score: f32,
}

impl<Id> Candidate<Id> {
    /// A hit on chunk `id` of the item at `item_uri`, scored `score` by its leg.
    pub fn new(
        id: Id,
        chunk_text: impl Into<String>,
        item_uri: impl Into<String>,
        item_title: impl Into<String>,
        score: f32,
    ) -> Self {
        Self {
            id,
            chunk_text: chunk_text.into(),
            item_uri: item_uri.into(),
            item_title: item_title.into(),
            score,
        }
    }
}
