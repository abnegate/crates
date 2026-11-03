use crate::pull_request::WebhookEvent;
use crate::pull_request::webhook_configuration::WebhookConfiguration;
use serde::Serialize;

/// The body of a request to create a repository webhook.
#[derive(Debug, Clone, Serialize)]
pub(super) struct WebhookRequest<'a> {
    pub(super) name: &'static str,
    pub(super) active: bool,
    pub(super) events: &'a [WebhookEvent],
    pub(super) config: WebhookConfiguration<'a>,
}
