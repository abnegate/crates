use serde::Serialize;

/// The verdict a review is submitted with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum ReviewEvent {
    /// No verdict: a comment that neither approves the change nor holds it
    /// back.
    Comment,
    /// Approval of the change as it stands.
    Approve,
    /// A request for changes before it merges.
    RequestChanges,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::MergeMethod;
    use serde_json::json;

    #[test]
    fn a_review_event_and_a_merge_method_are_spelled_as_github_spells_them() {
        for (event, spelled) in [
            (ReviewEvent::Comment, "COMMENT"),
            (ReviewEvent::Approve, "APPROVE"),
            (ReviewEvent::RequestChanges, "REQUEST_CHANGES"),
        ] {
            assert_eq!(serde_json::to_value(event).unwrap(), json!(spelled));
        }

        for (method, spelled) in [
            (MergeMethod::Squash, "squash"),
            (MergeMethod::Merge, "merge"),
            (MergeMethod::Rebase, "rebase"),
        ] {
            assert_eq!(serde_json::to_value(method).unwrap(), json!(spelled));
        }
    }
}
