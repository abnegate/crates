use crate::screening::Rejection;
use serde::Serialize;

/// An image left out of the training set.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct Dropped {
    /// The image's name, as submitted.
    pub filename: String,
    /// Why it was left out.
    pub reason: Rejection,
}
