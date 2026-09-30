/// The verdict a reviewer submitted with a review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReviewState {
    /// The reviewer approved the change.
    Approved,
    /// The reviewer asked for changes before it can merge.
    ChangesRequested,
    /// The reviewer commented without a verdict.
    Commented,
    /// An earlier verdict was withdrawn.
    Dismissed,
    /// The review is started but not yet submitted.
    Pending,
    /// A state this crate does not recognise.
    Unknown,
}

impl ReviewState {
    /// Read GitHub's name for a state, such as `CHANGES_REQUESTED`, ignoring
    /// case and surrounding whitespace. Anything unrecognised is
    /// [`Unknown`](Self::Unknown), never an error.
    pub fn parse(text: &str) -> Self {
        match text.trim().to_ascii_uppercase().as_str() {
            "APPROVED" => ReviewState::Approved,
            "CHANGES_REQUESTED" => ReviewState::ChangesRequested,
            "COMMENTED" => ReviewState::Commented,
            "DISMISSED" => ReviewState::Dismissed,
            "PENDING" => ReviewState::Pending,
            _ => ReviewState::Unknown,
        }
    }

    /// Whether this verdict replaces the reviewer's previous one.
    pub(super) fn settles(self) -> bool {
        matches!(
            self,
            ReviewState::Approved | ReviewState::ChangesRequested | ReviewState::Dismissed
        )
    }
}
