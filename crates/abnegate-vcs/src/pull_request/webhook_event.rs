use serde::Serialize;

/// A repository event a webhook is delivered for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WebhookEvent {
    /// An issue opened, edited, closed, reopened, labelled or deleted.
    Issues,
    /// A comment on an issue or a pull request's conversation.
    IssueComment,
    /// A pull request opened, edited, closed or synchronised.
    PullRequest,
    /// A push to a branch or a tag.
    Push,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_webhook_event_is_spelled_as_github_spells_it() {
        for (event, spelled) in [
            (WebhookEvent::Issues, "issues"),
            (WebhookEvent::IssueComment, "issue_comment"),
            (WebhookEvent::PullRequest, "pull_request"),
            (WebhookEvent::Push, "push"),
        ] {
            assert_eq!(serde_json::to_value(event).unwrap(), json!(spelled));
        }
    }
}
