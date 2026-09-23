use crate::screening::Rejection;
use serde::Serialize;

/// Which images to train on, and what the rest were rejected for. The two lists
/// partition the caller's slice: an image restored to keep the set at its floor
/// appears in `keep`, not in `drop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Verdict {
    pub keep: Vec<usize>,
    pub drop: Vec<(usize, Rejection)>,
}
