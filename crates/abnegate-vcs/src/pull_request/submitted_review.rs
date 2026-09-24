use crate::pull_request::ReviewState;

/// One submitted review, reduced to the two things the tally needs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SubmittedReview {
    /// The verdict submitted.
    pub state: ReviewState,
    /// Who submitted it, if GitHub says.
    pub reviewer: Option<String>,
}

impl SubmittedReview {
    /// A review with verdict `state`, submitted by `reviewer` when GitHub
    /// says who that was.
    pub fn new(state: ReviewState, reviewer: Option<String>) -> Self {
        Self { state, reviewer }
    }
}
