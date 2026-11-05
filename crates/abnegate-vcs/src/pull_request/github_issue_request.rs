use crate::pull_request::IssueState;
use crate::pull_request::IssueUpdate;
use serde::Serialize;

/// The body of a request to open an issue or to change one, naming only what
/// is to be set.
#[derive(Debug, Clone, Default, Serialize)]
pub(super) struct GitHubIssueRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) body: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) state: Option<IssueState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) labels: Option<&'a [String]>,
}

impl<'a> From<&'a IssueUpdate> for GitHubIssueRequest<'a> {
    fn from(update: &'a IssueUpdate) -> Self {
        Self {
            title: update.title.as_deref(),
            body: update.body.as_deref(),
            state: update.state,
            labels: update.labels.as_deref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn written(update: &IssueUpdate) -> serde_json::Value {
        serde_json::to_value(GitHubIssueRequest::from(update)).unwrap()
    }

    #[test]
    fn only_what_was_set_is_sent() {
        assert_eq!(written(&IssueUpdate::default()), json!({}));
        assert_eq!(
            written(&IssueUpdate::default().with_title("Renamed")),
            json!({ "title": "Renamed" })
        );
        assert_eq!(
            written(
                &IssueUpdate::default()
                    .with_body("Rewritten.")
                    .with_state(IssueState::Closed)
                    .with_labels(["bug", "docs"])
            ),
            json!({ "body": "Rewritten.", "state": "closed", "labels": ["bug", "docs"] })
        );
        assert_eq!(
            written(&IssueUpdate::default().with_labels(Vec::<String>::new())),
            json!({ "labels": [] })
        );
    }

    #[test]
    fn an_unknown_state_sends_no_state_at_all() {
        assert_eq!(
            written(
                &IssueUpdate::default()
                    .with_state(IssueState::Open)
                    .with_state(IssueState::Unknown)
            ),
            json!({})
        );
    }
}
