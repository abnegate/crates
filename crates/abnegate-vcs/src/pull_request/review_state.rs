/// The verdict a reviewer submitted with a review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
    Pending,
    Unknown,
}

impl ReviewState {
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
