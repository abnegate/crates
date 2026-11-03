use abnegate_secret::SecretValue;
use serde::Serialize;
use url::Url;

/// Where a webhook delivers, and the secret its deliveries are signed with.
///
/// Deliveries are always JSON, and always over verified TLS when the address
/// is HTTPS.
#[derive(Debug, Clone, Serialize)]
pub(super) struct WebhookConfiguration<'a> {
    pub(super) url: &'a str,
    pub(super) content_type: &'static str,
    pub(super) secret: &'a SecretValue,
    pub(super) insecure_ssl: &'static str,
}

impl<'a> WebhookConfiguration<'a> {
    pub(super) fn new(url: &'a Url, secret: &'a SecretValue) -> Self {
        Self {
            url: url.as_str(),
            content_type: "json",
            secret,
            insecure_ssl: "0",
        }
    }
}
