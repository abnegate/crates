use crate::pull_request::ReviewState;
use crate::pull_request::SubmittedReview;
use std::collections::HashMap;

/// What the submitted reviews add up to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReviewTally {
    /// Reviews that requested changes.
    pub cycles: u32,
    /// Distinct reviewers whose latest verdict was an approval.
    pub approvals: u32,
}

/// A round of review is a reviewer sending the change back, so every
/// `CHANGES_REQUESTED` costs a cycle. Approvals count distinct people whose
/// latest verdict was an approval: approving twice is still one approval, and a
/// reviewer who later requests changes has withdrawn theirs.
pub fn tally(reviews: &[SubmittedReview]) -> ReviewTally {
    let mut cycles = 0u32;
    let mut latest: HashMap<&str, ReviewState> = HashMap::new();
    let mut unattributed = 0u32;

    for review in reviews {
        if review.state == ReviewState::ChangesRequested {
            cycles = cycles.saturating_add(1);
        }

        let reviewer = review
            .reviewer
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());

        match reviewer {
            Some(name) if review.state.settles() => {
                latest.insert(name, review.state);
            }
            None if review.state == ReviewState::Approved => {
                unattributed = unattributed.saturating_add(1);
            }
            _ => {}
        }
    }

    let named = latest
        .values()
        .filter(|state| **state == ReviewState::Approved)
        .count();

    ReviewTally {
        cycles,
        approvals: u32::try_from(named)
            .unwrap_or(u32::MAX)
            .saturating_add(unattributed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review(state: &str, reviewer: Option<&str>) -> SubmittedReview {
        SubmittedReview {
            state: ReviewState::parse(state),
            reviewer: reviewer.map(str::to_string),
        }
    }

    #[test]
    fn every_request_for_changes_costs_a_review_cycle() {
        let tallied = tally(&[
            review("CHANGES_REQUESTED", Some("ada")),
            review("COMMENTED", Some("grace")),
            review("CHANGES_REQUESTED", Some("grace")),
        ]);
        assert_eq!(tallied.cycles, 2);
    }

    #[test]
    fn a_reviewer_who_approves_twice_is_still_one_approval() {
        let tallied = tally(&[
            review("APPROVED", Some("ada")),
            review("APPROVED", Some("ada")),
        ]);
        assert_eq!(tallied.approvals, 1);
        assert_eq!(tallied.cycles, 0);
    }

    #[test]
    fn an_approval_withdrawn_by_a_later_request_for_changes_stops_counting() {
        let tallied = tally(&[
            review("APPROVED", Some("ada")),
            review("CHANGES_REQUESTED", Some("ada")),
        ]);
        assert_eq!(
            tallied.approvals, 0,
            "a reviewer who came back asking for changes has withdrawn their approval"
        );
        assert_eq!(tallied.cycles, 1);
    }

    #[test]
    fn a_comment_only_review_is_neither_a_cycle_nor_an_approval() {
        let tallied = tally(&[review("COMMENTED", Some("ada")), review("PENDING", None)]);
        assert_eq!(tallied, ReviewTally::default());
    }

    #[test]
    fn an_unknown_review_state_is_counted_as_neither() {
        let tallied = tally(&[review("SOMETHING_NEW", Some("ada"))]);
        assert_eq!(tallied, ReviewTally::default());
        assert_eq!(ReviewState::parse("something_new"), ReviewState::Unknown);
    }
}
