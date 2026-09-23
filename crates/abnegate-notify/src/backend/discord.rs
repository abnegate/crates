//! Discord, over a channel webhook.

use async_trait::async_trait;
use chrono::SecondsFormat;
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

const HOSTS: &[&str] = &[
    "discord.com",
    "discordapp.com",
    "canary.discord.com",
    "ptb.discord.com",
];
const MAX_TITLE_CHARS: usize = 256;
const MAX_DESCRIPTION_CHARS: usize = 4_096;
const MAX_FIELD_NAME_CHARS: usize = 256;
const MAX_FIELD_VALUE_CHARS: usize = 1_024;
const MAX_FOOTER_CHARS: usize = 2_048;
const MAX_FIELDS: usize = 25;

/// Delivers to one Discord channel webhook.
#[derive(Debug)]
pub struct Discord {
    webhook: Webhook,
    name: Option<String>,
    footer: Option<String>,
}

impl Discord {
    /// Point at `webhook_url`, which must be a Discord webhook URL.
    pub fn new(webhook_url: &str) -> Result<Self, NotifyError> {
        let endpoint = Endpoint::new(webhook_url, HOSTS)?;
        Ok(Self {
            webhook: Webhook::new(endpoint, DEFAULT_TIMEOUT)?,
            name: None,
            footer: None,
        })
    }

    /// Label this instance, for a caller with more than one Discord hook.
    #[must_use]
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Sign every embed, usually with the sending application's name.
    #[must_use]
    pub fn footer(mut self, footer: impl Into<String>) -> Self {
        self.footer = Some(footer.into());
        self
    }

    #[cfg(test)]
    pub(crate) fn at_test_server(base_url: &str) -> Self {
        Self {
            webhook: Webhook::new(
                Endpoint::for_test(&format!("{base_url}/api/webhooks/1/secret")),
                DEFAULT_TIMEOUT,
            )
            .expect("test client"),
            name: None,
            footer: None,
        }
    }

    fn payload(&self, notification: &Notification) -> Value {
        let mut embed = json!({
            "title": truncate(notification.title(), MAX_TITLE_CHARS),
            "color": colour(notification.kind()),
            "timestamp": notification
                .timestamp()
                .to_rfc3339_opts(SecondsFormat::Secs, true),
        });

        if !notification.body().is_empty() {
            embed["description"] = json!(truncate(notification.body(), MAX_DESCRIPTION_CHARS));
        }

        if let Some(link) = notification.url() {
            embed["url"] = json!(link);
        }

        if let Some(footer) = &self.footer {
            embed["footer"] = json!({ "text": truncate(footer, MAX_FOOTER_CHARS) });
        }

        if !notification.fields().is_empty() {
            let fields: Vec<Value> = notification
                .fields()
                .iter()
                .take(MAX_FIELDS)
                .map(|field| {
                    json!({
                        "name": truncate(field.name(), MAX_FIELD_NAME_CHARS),
                        "value": truncate(field.value(), MAX_FIELD_VALUE_CHARS),
                        "inline": true,
                    })
                })
                .collect();
            embed["fields"] = json!(fields);
        }

        json!({ "embeds": [embed] })
    }
}

#[async_trait]
impl Notifier for Discord {
    fn channel(&self) -> Channel {
        Channel::DISCORD
    }

    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    async fn deliver(&self, notification: &Notification) -> Result<(), NotifyError> {
        self.webhook.post(&self.payload(notification)).await?;
        Ok(())
    }
}

/// Discord renders an embed's accent bar from a decimal RGB integer.
fn colour(severity: Severity) -> u32 {
    match severity {
        Severity::Info => 0x3498db,
        Severity::Success => 0x2ecc71,
        Severity::Warning => 0xf39c12,
        Severity::Error => 0xe74c3c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::EndpointError;
    use chrono::{TimeZone, Utc};
    use std::time::Duration;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn at(server: &MockServer) -> Discord {
        Discord::at_test_server(&server.uri())
    }

    fn pinned(title: &str, body: &str) -> Notification {
        Notification::new(title, body).at(Utc.with_ymd_and_hms(2026, 9, 8, 12, 0, 0).unwrap())
    }

    #[test]
    fn every_discord_host_is_accepted_and_others_are_not() {
        for host in HOSTS {
            let url = format!("https://{host}/api/webhooks/1/token");
            assert!(Discord::new(&url).is_ok(), "{host} should be allowed");
        }

        let error = Discord::new("https://discord.com.attacker.test/api/webhooks/1/token")
            .expect_err("lookalike host must be refused");
        assert!(matches!(
            error,
            NotifyError::Endpoint(EndpointError::HostNotAllowed { .. })
        ));
    }

    #[test]
    fn the_webhook_url_never_appears_in_debug() {
        let discord =
            Discord::new("https://discord.com/api/webhooks/1/xxxxSECRETxxxx").expect("valid");
        let rendered = format!("{discord:?}");
        assert!(!rendered.contains("xxxxSECRETxxxx"), "leaked: {rendered}");
        assert!(rendered.contains("discord.com"));
    }

    #[tokio::test]
    async fn the_payload_is_a_single_embed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/webhooks/1/secret"))
            .and(header("content-type", "application/json"))
            .and(body_json(json!({
                "embeds": [{
                    "title": "Build failed",
                    "color": 15158332,
                    "timestamp": "2026-09-08T12:00:00Z",
                    "description": "3 tests failed",
                    "url": "https://example.test/b/1",
                    "footer": { "text": "Builds" },
                    "fields": [
                        { "name": "Branch", "value": "main", "inline": true },
                        { "name": "Commit", "value": "abc", "inline": true }
                    ]
                }]
            })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let notification = pinned("Build failed", "3 tests failed")
            .severity(Severity::Error)
            .link("https://example.test/b/1")
            .field("Branch", "main")
            .field("Commit", "abc");

        at(&server)
            .footer("Builds")
            .deliver(&notification)
            .await
            .expect("delivered");
    }

    #[tokio::test]
    async fn optional_parts_are_omitted_rather_than_sent_as_null() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_json(json!({
                "embeds": [{
                    "title": "Heads up",
                    "color": 3447003,
                    "timestamp": "2026-09-08T12:00:00Z",
                }]
            })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        at(&server)
            .deliver(&pinned("Heads up", ""))
            .await
            .expect("delivered");
    }

    #[test]
    fn each_severity_maps_to_its_own_colour() {
        assert_eq!(colour(Severity::Info), 3447003);
        assert_eq!(colour(Severity::Success), 3066993);
        assert_eq!(colour(Severity::Warning), 15965202);
        assert_eq!(colour(Severity::Error), 15158332);
    }

    #[test]
    fn discords_field_ceiling_is_respected() {
        let mut notification = pinned("Many", "");
        for index in 0..40 {
            notification = notification.field(format!("Key {index}"), "value");
        }

        let discord = Discord::new("https://discord.com/api/webhooks/1/token").expect("valid");
        let payload = discord.payload(&notification);
        assert_eq!(
            payload["embeds"][0]["fields"]
                .as_array()
                .expect("fields")
                .len(),
            MAX_FIELDS
        );
    }

    #[test]
    fn over_long_text_is_cut_to_discords_limits() {
        let notification =
            pinned(&"T".repeat(500), &"B".repeat(9_000)).field("N".repeat(400), "V".repeat(2_000));

        let discord = Discord::new("https://discord.com/api/webhooks/1/token")
            .expect("valid")
            .footer("F".repeat(3_000));
        let embed = discord.payload(&notification);
        let embed = &embed["embeds"][0];

        assert_eq!(
            embed["title"].as_str().unwrap().chars().count(),
            MAX_TITLE_CHARS
        );
        assert_eq!(
            embed["description"].as_str().unwrap().chars().count(),
            MAX_DESCRIPTION_CHARS
        );
        assert_eq!(
            embed["fields"][0]["name"].as_str().unwrap().chars().count(),
            MAX_FIELD_NAME_CHARS
        );
        assert_eq!(
            embed["fields"][0]["value"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            MAX_FIELD_VALUE_CHARS
        );
        assert_eq!(
            embed["footer"]["text"].as_str().unwrap().chars().count(),
            MAX_FOOTER_CHARS
        );
    }

    #[test]
    fn an_embed_carries_no_footer_until_one_is_set() {
        let discord = Discord::new("https://discord.com/api/webhooks/1/token").expect("valid");
        assert!(discord.payload(&pinned("Title", "Body"))["embeds"][0]["footer"].is_null());
    }

    #[tokio::test]
    async fn a_rate_limit_is_reported_with_the_wait_discord_asked_for() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "2"))
            .mount(&server)
            .await;

        let error = at(&server)
            .deliver(&pinned("Title", "Body"))
            .await
            .expect_err("rate limited");

        assert_eq!(error.retry_after(), Some(Duration::from_secs(2)));
        assert!(error.is_retryable());
        assert!(!error.to_string().contains("secret"));
    }
}
