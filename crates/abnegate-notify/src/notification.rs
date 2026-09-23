//! What gets delivered.

use abnegate_secret::sanitize_owned;
use chrono::{DateTime, Utc};
use url::Url;

use crate::field::Field;
use crate::severity::Severity;

/// One message, on its way to every channel at once.
///
/// Fields are private because the text is sanitized on the way in and that
/// invariant has to hold for every backend. A notification body carries tool
/// output and user data, so it passes through [`abnegate_secret::sanitize`],
/// which strips terminal control sequences and redacts credentials. A chat
/// channel's history is far harder to scrub than a log file, and control
/// sequences in a message can forge output in any terminal-based reader, so
/// sanitizing happens here, once, rather than in each backend where it could
/// be forgotten.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    title: String,
    body: String,
    severity: Severity,
    link: Option<String>,
    fields: Vec<Field>,
    timestamp: DateTime<Utc>,
}

impl Notification {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: clean(title.into()),
            body: clean(body.into()),
            severity: Severity::default(),
            link: None,
            fields: Vec::new(),
            timestamp: Utc::now(),
        }
    }

    #[must_use]
    pub fn severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// Attach a link, which is kept only if it is an `http(s)` URL.
    ///
    /// Anything else is dropped rather than rejected: a `javascript:` or
    /// `data:` target is worth refusing, but not at the cost of failing an
    /// otherwise deliverable notification. Unlike the text, a link is not
    /// redacted, because it is built by the application rather than taken
    /// from tool output.
    #[must_use]
    pub fn link(mut self, link: impl Into<String>) -> Self {
        let link = link.into();
        self.link = Url::parse(&link)
            .ok()
            .filter(|parsed| matches!(parsed.scheme(), "http" | "https"))
            .map(String::from);
        self
    }

    #[must_use]
    pub fn field(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.push(Field::new(name, value));
        self
    }

    #[must_use]
    pub fn at(mut self, timestamp: DateTime<Utc>) -> Self {
        self.timestamp = timestamp;
        self
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn kind(&self) -> Severity {
        self.severity
    }

    pub fn url(&self) -> Option<&str> {
        self.link.as_deref()
    }

    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    pub fn timestamp(&self) -> DateTime<Utc> {
        self.timestamp
    }

    /// The whole notification as plain text.
    ///
    /// Every channel can render this, so a backend that has no richer format
    /// still says everything the notification carries.
    pub fn to_plain_text(&self) -> String {
        let mut rendered = self.title.clone();
        if !self.body.is_empty() {
            rendered.push_str("\n\n");
            rendered.push_str(&self.body);
        }
        for field in &self.fields {
            rendered.push_str("\n\n");
            rendered.push_str(field.name());
            rendered.push_str(": ");
            rendered.push_str(field.value());
        }
        if let Some(link) = &self.link {
            rendered.push_str("\n\n");
            rendered.push_str(link);
        }
        rendered
    }
}

pub(crate) fn clean(text: String) -> String {
    sanitize_owned(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn a_notification_keeps_what_it_was_given() {
        let notification = Notification::new("Build failed", "3 tests failed")
            .severity(Severity::Error)
            .link("https://example.test/builds/1")
            .field("Branch", "main");

        assert_eq!(notification.title(), "Build failed");
        assert_eq!(notification.body(), "3 tests failed");
        assert_eq!(notification.kind(), Severity::Error);
        assert_eq!(notification.url(), Some("https://example.test/builds/1"));
        assert_eq!(notification.fields().len(), 1);
        assert_eq!(notification.fields()[0].name(), "Branch");
    }

    #[test]
    fn terminal_control_sequences_are_stripped_from_the_body() {
        let notification = Notification::new(
            "\u{1b}[31mBuild failed\u{1b}[0m",
            "\u{1b}]0;stolen title\u{7}done",
        );
        assert_eq!(notification.title(), "Build failed");
        assert_eq!(notification.body(), "done");
    }

    #[test]
    fn a_credential_in_the_body_is_redacted_before_it_leaves() {
        let notification = Notification::new(
            "Deploy log",
            concat!("export GITHUB_TOKEN=ghp_", "0123456789abcdefghij"),
        );
        assert!(
            !notification
                .body()
                .contains(concat!("ghp_", "0123456789abcdefghij"))
        );
        assert!(notification.body().contains("[REDACTED]"));
    }

    #[test]
    fn a_credential_in_a_field_is_redacted_too() {
        let notification = Notification::new("Deploy", "")
            .field("Env", concat!("OPENAI_KEY=sk-", "0123456789abcdefghij"));
        assert!(!notification.fields()[0].value().contains("sk-0123456789"));
        assert!(notification.fields()[0].value().contains("[REDACTED]"));
    }

    /// The reason [`Notification::link`] does not redact.
    ///
    /// A one-time token in a query parameter is exactly what the redactor is
    /// built to catch, so an account-flow URL survives as a link and does not
    /// survive being interpolated into the body.
    #[test]
    fn a_one_time_token_survives_as_a_link_but_not_in_the_body() {
        let reset = "https://example.test/reset?token=8Xk2Qm9Lp4Rw7Tz1Vb6Nh3Yj5Fd0Gs8Ac2Ee4Ii6Ko";
        let notification =
            Notification::new("Reset your password", format!("Open {reset}")).link(reset);

        assert_eq!(notification.url(), Some(reset));
        assert!(
            notification.body().contains("[REDACTED]"),
            "body was {}",
            notification.body()
        );
    }

    #[test]
    fn a_non_http_link_is_dropped() {
        for link in [
            "javascript:alert(1)",
            "data:text/html,<script>",
            "file:///etc/passwd",
            "not a url",
        ] {
            let notification = Notification::new("Title", "Body").link(link);
            assert_eq!(notification.url(), None, "{link} should have been dropped");
        }
    }

    #[test]
    fn plain_text_carries_every_part() {
        let rendered = Notification::new("Build failed", "3 tests failed")
            .field("Branch", "main")
            .link("https://example.test/builds/1")
            .to_plain_text();

        assert!(rendered.starts_with("Build failed"));
        assert!(rendered.contains("3 tests failed"));
        assert!(rendered.contains("Branch: main"));
        assert!(rendered.ends_with("https://example.test/builds/1"));
    }

    #[test]
    fn plain_text_omits_an_absent_body_and_link() {
        assert_eq!(
            Notification::new("Just a title", "").to_plain_text(),
            "Just a title"
        );
    }

    #[test]
    fn the_timestamp_can_be_pinned() {
        let moment = Utc.with_ymd_and_hms(2026, 9, 8, 12, 0, 0).unwrap();
        assert_eq!(
            Notification::new("Title", "Body").at(moment).timestamp(),
            moment
        );
    }
}
