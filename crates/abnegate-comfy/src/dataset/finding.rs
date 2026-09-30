use crate::dataset::Concern;
use serde::Serialize;

/// One advisory about a training set, which the caller may overrule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Finding {
    /// What looks wrong.
    pub concern: Concern,
    /// What to do about it, in words for the person who chose the images.
    pub detail: String,
}
