use serde::Deserialize;
use serde::Serialize;

/// Whether an issue is still open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum IssueState {
    /// Still open.
    Open,
    /// Closed, as completed or not.
    Closed,
    /// A state this crate does not recognise, read from any other wire value
    /// or from none.
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_state_github_has_not_named_yet_is_unknown() {
        for (word, state) in [
            ("open", IssueState::Open),
            ("closed", IssueState::Closed),
            ("archived", IssueState::Unknown),
        ] {
            assert_eq!(
                serde_json::from_value::<IssueState>(json!(word)).unwrap(),
                state
            );
        }
    }
}
