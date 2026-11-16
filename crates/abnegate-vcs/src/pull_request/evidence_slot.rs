use crate::pull_request::PullRequestEvidence;

/// What
/// [`PullRequestService::fetch_pull_request_evidence`](crate::pull_request::PullRequestService::fetch_pull_request_evidence)
/// read for one of the pull requests it was asked for: its evidence, or why
/// there is none, in which case a caller reads that pull request another
/// way. No variant but [`Self::Read`] fails the others asked for with it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EvidenceSlot {
    /// Everything that says whether it is ready to merge.
    Read(Box<PullRequestEvidence>),
    /// GitHub reported an error at it that concerns it alone, such as one it
    /// could not find. A refusal of what the token may read fails the whole
    /// call instead, wherever GitHub reports it.
    Reported,
    /// GitHub sent nothing for it.
    Missing,
    /// GitHub answered for another pull request, or for one in another
    /// repository.
    Foreign,
    /// GitHub answered with something this crate cannot read as a pull
    /// request: another kind of node, a field it always sends left out, or a
    /// branch or commit this crate does not accept.
    Unreadable,
    /// GitHub could not answer for it: every request that asked for it ran
    /// out of time or answered with more than this crate reads.
    Unanswered,
}

impl EvidenceSlot {
    /// Why no evidence was read, in a word for a log line, or `None` when it
    /// was.
    pub(super) fn reason(&self) -> Option<&'static str> {
        match self {
            Self::Read(_) => None,
            Self::Reported => Some("reported"),
            Self::Missing => Some("missing"),
            Self::Foreign => Some("foreign"),
            Self::Unreadable => Some("unreadable"),
            Self::Unanswered => Some("unanswered"),
        }
    }

    /// The evidence read, if it was.
    pub fn evidence(&self) -> Option<&PullRequestEvidence> {
        match self {
            Self::Read(evidence) => Some(evidence),
            _ => None,
        }
    }

    /// The evidence read, if it was, taken out of the slot.
    pub fn into_evidence(self) -> Option<PullRequestEvidence> {
        match self {
            Self::Read(evidence) => Some(*evidence),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_read_slot_holds_evidence() {
        for slot in [
            EvidenceSlot::Reported,
            EvidenceSlot::Missing,
            EvidenceSlot::Foreign,
            EvidenceSlot::Unreadable,
            EvidenceSlot::Unanswered,
        ] {
            assert_eq!(slot.evidence(), None, "{slot:?}");
            assert_eq!(slot.into_evidence(), None);
        }
    }
}
