/// What the checks and statuses reported on one commit add up to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChecksOutcome {
    /// Every check and status finished without failing.
    Success,
    /// At least one failed; the names say which, sorted and without repeats.
    Failure(Vec<String>),
    /// None has failed, but at least one is still running or not every one
    /// could be read.
    Pending,
    /// Nothing reports on this commit at all.
    Absent,
}

impl ChecksOutcome {
    /// The outcome as the one word a caller stores: `success`, `failure`,
    /// `pending` or `absent`.
    pub fn label(&self) -> &'static str {
        match self {
            ChecksOutcome::Success => "success",
            ChecksOutcome::Failure(_) => "failure",
            ChecksOutcome::Pending => "pending",
            ChecksOutcome::Absent => "absent",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_outcome_labels_are_the_words_a_caller_stores() {
        for (outcome, label) in [
            (ChecksOutcome::Success, "success"),
            (ChecksOutcome::Failure(vec!["build".to_string()]), "failure"),
            (ChecksOutcome::Failure(Vec::new()), "failure"),
            (ChecksOutcome::Pending, "pending"),
            (ChecksOutcome::Absent, "absent"),
        ] {
            assert_eq!(outcome.label(), label, "{outcome:?}");
        }
    }
}
