//! Slack, over an incoming webhook.

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::backend::webhook::Webhook;
use crate::channel::Channel;
use crate::endpoint::Endpoint;
use crate::error::NotifyError;
use crate::fanout::DEFAULT_TIMEOUT;
use crate::notification::Notification;
use crate::notifier::Notifier;
use crate::severity::Severity;
use crate::text::truncate;

const HOSTS: &[&str] = &["hooks.slack.com"];
const LINK_LABEL: &str = "Open";
const MAX_HEADER_CHARS: usize = 150;
const MAX_SECTION_CHARS: usize = 3_000;
const MAX_FIELD_CHARS: usize = 2_000;
const MAX_FIELDS_PER_SECTION: usize = 10;

/// Delivers to one Slack incoming webhook.
#[derive(Debug)]
pub struct Slack {
    webhook: Webhook,
    name: Option<String>,
}

impl Slack {
    /// Point at `webhook_url`, which must be a `hooks.slack.com` URL.
    pub fn new(webhook_url: &str) -> Result<Self, NotifyError> {
        let endpoint = Endpoint::new(webhook_url, HOSTS)?;
        Ok(Self {
            webhook: Webhook::new(endpoint, DEFAULT_TIMEOUT)?,
            name: None,
        })
    }

    /// Label this instance, for a caller with more than one Slack hook.
    #[must_use]
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    #[cfg(test)]
    pub(crate) fn at_test_server(base_url: &str) -> Self {
        Self {
            webhook: Webhook::new(
                Endpoint::for_test(&format!("{base_url}/services/T000/B000/secret")),
                DEFAULT_TIMEOUT,
            )
            .expect("test client"),
            name: None,
        }
    }

    fn payload(&self, notification: &Notification) -> Value {
        let mut blocks = vec![json!({
            "type": "header",
            "text": {
                "type": "plain_text",
                "text": truncate(
                    &format!("{} {}", icon(notification.kind()), escape(notification.title())),
                    MAX_HEADER_CHARS,
                ),
                "emoji": true,
            }
        })];

        if !notification.body().is_empty() {
            blocks.push(json!({
                "type": "section",
                "text": {
                    "type": "mrkdwn",
                    "text": truncate(&escape(notification.body()), MAX_SECTION_CHARS),
                }
            }));
        }

        for chunk in notification.fields().chunks(MAX_FIELDS_PER_SECTION) {
            let fields: Vec<Value> = chunk
                .iter()
                .map(|field| {
                    json!({
                        "type": "mrkdwn",
                        "text": truncate(
                            &format!("*{}*\n{}", escape(field.name()), escape(field.value())),
                            MAX_FIELD_CHARS,
                        ),
                    })
                })
                .collect();
            blocks.push(json!({ "type": "section", "fields": fields }));
        }

        if let Some(link) = notification.url() {
            blocks.push(json!({
                "type": "context",
                "elements": [{
                    "type": "mrkdwn",
                    "text": format!("<{}|{LINK_LABEL}>", escape(link)),
                }],
            }));
        }

        json!({
            "text": truncate(&escape(&notification.to_plain_text()), MAX_SECTION_CHARS),
            "blocks": blocks,
        })
    }
}

#[async_trait]
impl Notifier for Slack {
    fn channel(&self) -> Channel {
        Channel::SLACK
    }

    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    async fn deliver(&self, notification: &Notification) -> Result<(), NotifyError> {
        self.webhook.post(&self.payload(notification)).await?;
        Ok(())
    }
}

fn icon(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => ":information_source:",
        Severity::Success => ":white_check_mark:",
        Severity::Warning => ":warning:",
        Severity::Error => ":rotating_light:",
    }
}

/// Slack's three reserved mrkdwn characters, escaped in the order it requires.
///
/// `&` has to go first or the ampersands introduced by the other two would be
/// escaped a second time.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::EndpointError;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn at(server: &MockServer) -> Slack {
        Slack::at_test_server(&server.uri())
    }

    #[test]
    fn ampersands_are_escaped_before_the_angle_brackets() {
        assert_eq!(escape("a & b"), "a &amp; b");
        assert_eq!(escape("<script>"), "&lt;script&gt;");
        assert_eq!(escape("Tom & <Jerry>"), "Tom &amp; &lt;Jerry&gt;");
        assert!(!escape("<a>").contains("&amp;lt;"));
    }

    #[test]
    fn a_url_outside_the_allowlist_is_refused() {
        let error = Slack::new("https://hooks.example.com/services/T/B/x")
            .expect_err("only Slack hosts are allowed");
        assert!(matches!(
            error,
            NotifyError::Endpoint(EndpointError::HostNotAllowed { .. })
        ));
    }

    #[test]
    fn a_slack_webhook_url_is_accepted() {
        assert!(Slack::new("https://hooks.slack.com/services/T000/B000/xyz").is_ok());
    }

    #[test]
    fn the_webhook_url_never_appears_in_debug() {
        let slack =
            Slack::new("https://hooks.slack.com/services/T000/B000/xxxxSECRETxxxx").expect("valid");
        let rendered = format!("{slack:?}");
        assert!(!rendered.contains("xxxxSECRETxxxx"), "leaked: {rendered}");
        assert!(rendered.contains("hooks.slack.com"));
    }

    #[tokio::test]
    async fn the_payload_is_block_kit_with_a_plain_text_fallback() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/services/T000/B000/secret"))
            .and(header("content-type", "application/json"))
            .and(body_json(json!({
                "text": "Build failed\n\n3 tests failed\n\nBranch: main\n\nhttps://example.test/b/1",
                "blocks": [
                    {
                        "type": "header",
                        "text": {
                            "type": "plain_text",
                            "text": ":rotating_light: Build failed",
                            "emoji": true,
                        }
                    },
                    {
                        "type": "section",
                        "text": { "type": "mrkdwn", "text": "3 tests failed" }
                    },
                    {
                        "type": "section",
                        "fields": [{ "type": "mrkdwn", "text": "*Branch*\nmain" }]
                    },
                    {
                        "type": "context",
                        "elements": [{
                            "type": "mrkdwn",
                            "text": "<https://example.test/b/1|Open>"
                        }]
                    }
                ]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&server)
            .await;

        let notification = Notification::new("Build failed", "3 tests failed")
            .severity(Severity::Error)
            .field("Branch", "main")
            .link("https://example.test/b/1");

        at(&server).deliver(&notification).await.expect("delivered");
    }

    #[tokio::test]
    async fn a_notification_with_no_body_omits_the_body_section() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_json(json!({
                "text": "Heads up",
                "blocks": [{
                    "type": "header",
                    "text": {
                        "type": "plain_text",
                        "text": ":information_source: Heads up",
                        "emoji": true,
                    }
                }]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&server)
            .await;

        at(&server)
            .deliver(&Notification::new("Heads up", ""))
            .await
            .expect("delivered");
    }

    #[tokio::test]
    async fn user_text_cannot_forge_slack_markup() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let payload = at(&server).payload(&Notification::new(
            "Deploy",
            "<https://evil.test|click me> & <@U000>",
        ));
        let body = payload["blocks"][1]["text"]["text"]
            .as_str()
            .expect("body section");

        assert_eq!(
            body,
            "&lt;https://evil.test|click me&gt; &amp; &lt;@U000&gt;"
        );

        let fallback = payload["text"].as_str().expect("fallback text");
        assert!(
            !fallback.contains("<https://evil.test|"),
            "the notification preview is mrkdwn too: {fallback}"
        );
        assert!(fallback.contains("&lt;https://evil.test|click me&gt;"));
    }

    #[test]
    fn more_than_ten_fields_are_split_across_sections() {
        let mut notification = Notification::new("Many", "");
        for index in 0..23 {
            notification = notification.field(format!("Key {index}"), "value");
        }

        let server_free = Slack::new("https://hooks.slack.com/services/T/B/x").expect("valid");
        let payload = server_free.payload(&notification);
        let blocks = payload["blocks"].as_array().expect("blocks");

        let sections: Vec<usize> = blocks
            .iter()
            .filter_map(|block| block["fields"].as_array().map(Vec::len))
            .collect();
        assert_eq!(sections, vec![10, 10, 3]);
    }

    #[test]
    fn an_over_long_title_is_truncated_to_slacks_header_limit() {
        let notification = Notification::new("T".repeat(400), "");
        let slack = Slack::new("https://hooks.slack.com/services/T/B/x").expect("valid");
        let header = slack.payload(&notification)["blocks"][0]["text"]["text"]
            .as_str()
            .expect("header")
            .chars()
            .count();
        assert_eq!(header, MAX_HEADER_CHARS);
    }

    #[tokio::test]
    async fn a_rejection_becomes_a_failure_that_names_only_the_host() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string("invalid_payload"))
            .mount(&server)
            .await;

        let error = at(&server)
            .deliver(&Notification::new("Title", "Body"))
            .await
            .expect_err("rejected");

        let rendered = error.to_string();
        assert!(rendered.contains("invalid_payload"));
        assert!(!rendered.contains("secret"), "leaked: {rendered}");
    }
}
