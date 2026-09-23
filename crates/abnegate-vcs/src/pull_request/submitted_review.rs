use crate::pull_request::ReviewState;

/// One submitted review, reduced to the two things the tally needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedReview {
    pub state: ReviewState,
    pub reviewer: Option<String>,
}
