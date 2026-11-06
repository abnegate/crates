//! A category chosen by [`Category::nearest_with_margin`](crate::Category::nearest_with_margin).

use crate::category::Category;

/// A category chosen by [`Category::nearest_with_margin`](crate::Category::nearest_with_margin),
/// with the evidence that chose it.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct CategoryMatch<'a> {
    /// The winning reference: the closest exemplar of the winning category.
    pub category: &'a Category,
    /// Cosine similarity to that exemplar, in `-1.0..=1.0`.
    pub score: f64,
    /// How far `score` leads the best exemplar of any other category. Equal to
    /// `score` when no other category was scored.
    pub margin: f64,
}
