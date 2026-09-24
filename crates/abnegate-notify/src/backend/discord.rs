//! Discord, over a channel webhook.

use std::time::Duration;

use async_trait::async_trait;
use chrono::SecondsFormat;
use serde_json::{Value, json};

use crate::backend::webhook::Webhook;
use crate::channel::Channel;
use crate::endpoint::Endpoint;
use crate::error::Error;
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
const MAX_EMBED_CHARS: usize = 6_000;
const MIN_FIELD_CHARS: usize = 2;

/// Delivers to one Discord channel webhook.
#[derive(Debug)]
pub struct Discord {
    webhook: Webhook,
    name: Option<String>,
    footer: Option<String>,
}

impl Discord {
    /// Point at `webhook_url`, which must be a Discord webhook URL.
    pub fn new(webhook_url: &str) -> Result<Self, Error> {
        let endpoint = Endpoint::new(webhook_url, HOSTS)?;
        Ok(Self {
            webhook: Webhook::new(endpoint)?,
            name: None,
            footer: None,
        })
    }

    /// Give up on a delivery after `timeout` rather than
    /// [`DEFAULT_TIMEOUT`](crate::DEFAULT_TIMEOUT).
    ///
    /// The same budget replaces the fan-out's for this channel, so a fan-out
    /// allowing longer than the default needs it set here too.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.webhook.set_timeout(timeout);
        self
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
            webhook: Webhook::new(Endpoint::for_test(&format!(
                "{base_url}/api/webhooks/1/secret"
            )))
            .expect("test client"),
            name: None,
            footer: None,
        }
    }

    /// One embed for `notification`, within Discord's per-part limits and its
    /// ceiling on the embed's text as a whole.
    ///
    /// The title, footer, description and fields draw on that ceiling in that
    /// order, so it is the fields that give way first.
    fn payload(&self, notification: &Notification) -> Value {
        let mut remaining = MAX_EMBED_CHARS;
        let mut embed = json!({
            "color": colour(notification.kind()),
            "timestamp": notification
                .timestamp()
                .to_rfc3339_opts(SecondsFormat::Secs, true),
        });

        if let Some(title) = spend(notification.title(), MAX_TITLE_CHARS, &mut remaining) {
            embed["title"] = json!(title);
        }

        if let Some(footer) = &self.footer
            && let Some(text) = spend(footer, MAX_FOOTER_CHARS, &mut remaining)
        {
            embed["footer"] = json!({ "text": text });
        }

        if !notification.body().is_empty()
            && let Some(description) =
                spend(notification.body(), MAX_DESCRIPTION_CHARS, &mut remaining)
        {
            embed["description"] = json!(description);
        }

        if let Some(link) = notification.url() {
            embed["url"] = json!(link);
        }

        let mut fields: Vec<Value> = Vec::new();
        for field in notification.fields().iter().take(MAX_FIELDS) {
            if remaining < MIN_FIELD_CHARS {
                break;
            }
            let name_limit = MAX_FIELD_NAME_CHARS.min(remaining - 1);
            let (Some(name), Some(value)) = (
                spend(field.name(), name_limit, &mut remaining),
                spend(field.value(), MAX_FIELD_VALUE_CHARS, &mut remaining),
            ) else {
                break;
            };
            fields.push(json!({ "name": name, "value": value, "inline": true }));
        }
        if !fields.is_empty() {
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

    fn timeout(&self) -> Option<Duration> {
        self.webhook.timeout()
    }

    async fn deliver(&self, notification: &Notification) -> Result<(), Error> {
        self.webhook.post(&self.payload(notification)).await?;
        Ok(())
    }
}

/// `text` cut to `limit` and to what is left of the embed's ceiling, which it
/// then draws down. Nothing is left to give once the ceiling is spent.
fn spend(text: &str, limit: usize, remaining: &mut usize) -> Option<String> {
    let allowed = limit.min(*remaining);
    if allowed == 0 {
        return None;
    }
    let spent = truncate(text, allowed);
    *remaining -= spent.chars().count();
    Some(spent)
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
            Error::Endpoint(EndpointError::HostNotAllowed { .. })
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

    fn embed(discord: &Discord, notification: &Notification) -> Value {
        discord.payload(notification)["embeds"][0].clone()
    }

    fn chars(value: &Value) -> usize {
        value.as_str().map_or(0, |text| text.chars().count())
    }

    /// Every part of an embed that counts towards Discord's 6000-character total.
    fn embed_chars(embed: &Value) -> usize {
        let fields: usize = embed["fields"].as_array().map_or(0, |fields| {
            fields
                .iter()
                .map(|field| chars(&field["name"]) + chars(&field["value"]))
                .sum()
        });
        chars(&embed["title"])
            + chars(&embed["description"])
            + chars(&embed["footer"]["text"])
            + fields
    }

    #[test]
    fn over_long_text_is_cut_to_each_parts_limit() {
        let discord = Discord::new("https://discord.com/api/webhooks/1/token").expect("valid");

        let title = embed(&discord, &pinned(&"T".repeat(500), ""));
        assert_eq!(chars(&title["title"]), MAX_TITLE_CHARS);

        let description = embed(&discord, &pinned("Title", &"B".repeat(9_000)));
        assert_eq!(chars(&description["description"]), MAX_DESCRIPTION_CHARS);

        let field = embed(
            &discord,
            &pinned("Title", "").field("N".repeat(400), "V".repeat(2_000)),
        );
        assert_eq!(chars(&field["fields"][0]["name"]), MAX_FIELD_NAME_CHARS);
        assert_eq!(chars(&field["fields"][0]["value"]), MAX_FIELD_VALUE_CHARS);

        let footer = embed(&discord.footer("F".repeat(3_000)), &pinned("Title", ""));
        assert_eq!(chars(&footer["footer"]["text"]), MAX_FOOTER_CHARS);
    }

    #[test]
    fn the_whole_embed_stays_within_discords_total() {
        let mut notification = pinned(&"T".repeat(500), &"B".repeat(9_000));
        for index in 0..MAX_FIELDS {
            notification = notification.field(format!("{index}").repeat(400), "V".repeat(2_000));
        }
        let discord = Discord::new("https://discord.com/api/webhooks/1/token")
            .expect("valid")
            .footer("F".repeat(3_000));

        let embed = embed(&discord, &notification);
        assert!(
            embed_chars(&embed) <= MAX_EMBED_CHARS,
            "the embed carries {} characters",
            embed_chars(&embed)
        );
        assert_eq!(chars(&embed["title"]), MAX_TITLE_CHARS);
        assert_eq!(chars(&embed["footer"]["text"]), MAX_FOOTER_CHARS);
    }

    #[test]
    fn fields_fill_whatever_the_total_leaves() {
        let mut notification = pinned("Title", &"B".repeat(4_000));
        for index in 0..MAX_FIELDS {
            notification = notification.field(format!("Key {index}"), "V".repeat(1_000));
        }
        let discord = Discord::new("https://discord.com/api/webhooks/1/token").expect("valid");

        let embed = embed(&discord, &notification);
        let fields = embed["fields"].as_array().expect("fields");
        assert!(!fields.is_empty() && fields.len() < MAX_FIELDS);
        for field in fields {
            assert!(chars(&field["name"]) >= 1 && chars(&field["value"]) >= 1);
        }
        assert!(embed_chars(&embed) <= MAX_EMBED_CHARS);
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

    #[tokio::test]
    async fn a_timeout_bounds_the_request_and_is_offered_to_the_fanout() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(5)))
            .mount(&server)
            .await;

        let discord = at(&server).timeout(Duration::from_millis(100));
        assert_eq!(
            Notifier::timeout(&discord),
            Some(Duration::from_millis(100))
        );

        let error = discord
            .deliver(&pinned("Title", "Body"))
            .await
            .expect_err("too slow");
        assert_eq!(
            error,
            Error::Timeout {
                after: Duration::from_millis(100)
            }
        );
    }
}
