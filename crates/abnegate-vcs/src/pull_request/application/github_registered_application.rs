use crate::pull_request::application::ApplicationError;
use crate::pull_request::application::ApplicationId;
use crate::pull_request::application::RegisteredApplication;
use crate::pull_request::application::github_account::GitHubAccount;
use abnegate_secret::SecretValue;
use serde::Deserialize;

/// A GitHub App a manifest registered, as GitHub's conversion of the
/// manifest's code answers with it. Every credential is read straight into a
/// [`SecretValue`], so none is ever held, or rendered, as a plain `String`.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubRegisteredApplication {
    pub(super) id: ApplicationId,
    pub(super) slug: String,
    pub(super) name: String,
    #[serde(default)]
    pub(super) owner: Option<GitHubAccount>,
    pub(super) html_url: String,
    pub(super) client_id: String,
    pub(super) client_secret: SecretValue,
    #[serde(default)]
    pub(super) webhook_secret: Option<SecretValue>,
    pub(super) pem: SecretValue,
}

impl TryFrom<GitHubRegisteredApplication> for RegisteredApplication {
    type Error = ApplicationError;

    /// The App `wire` describes, owned by the account of the kind its owner
    /// names. An empty webhook secret is none at all.
    ///
    /// No owner, an owner [`GitHubAccount::account`] refuses, or an empty
    /// client secret or private key is [`ApplicationError::Response`].
    fn try_from(wire: GitHubRegisteredApplication) -> Result<Self, Self::Error> {
        let owner = wire
            .owner
            .ok_or(ApplicationError::Response)?
            .account(None)?;
        if wire.client_secret.is_empty() || wire.pem.is_empty() {
            return Err(ApplicationError::Response);
        }
        let registered = RegisteredApplication::new(
            wire.id,
            wire.slug,
            wire.name,
            owner,
            wire.html_url,
            wire.client_id,
            wire.client_secret,
            wire.pem,
        );
        Ok(
            match wire.webhook_secret.filter(|secret| !secret.is_empty()) {
                Some(webhook_secret) => registered.with_webhook_secret(webhook_secret),
                None => registered,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::application::Account;
    use crate::pull_request::application::AccountType;
    use abnegate_secret::OptionalSecretExtension;
    use serde_json::Value;
    use serde_json::json;

    /// The client secret every row carries, to see where it travels.
    const CLIENT_SECRET: &str = "client-secret-sentinel";

    /// The webhook secret every row carries, to see where it travels.
    const WEBHOOK_SECRET: &str = "webhook-secret-sentinel";

    /// The private key every row carries, to see where it travels.
    const PEM: &str =
        "-----BEGIN RSA PRIVATE KEY-----\npem-sentinel\n-----END RSA PRIVATE KEY-----";

    fn row() -> Value {
        json!({
            "id": 12345,
            "slug": "octoapp",
            "node_id": "MDxOkludGVncmF0aW9uMQ==",
            "owner": { "login": "octocat-org", "id": 1, "type": "Organization" },
            "name": "Octocat App",
            "description": "",
            "external_url": "https://example.com",
            "html_url": "https://github.com/apps/octoapp",
            "created_at": "2017-07-08T16:18:44-04:00",
            "updated_at": "2017-07-08T16:18:44-04:00",
            "permissions": { "metadata": "read", "contents": "read" },
            "events": ["push", "pull_request"],
            "client_id": "Iv1.8a61f9b3a7aba766",
            "client_secret": CLIENT_SECRET,
            "webhook_secret": WEBHOOK_SECRET,
            "pem": PEM
        })
    }

    fn with(field: &str, value: Value) -> Value {
        let mut row = row();
        row[field] = value;
        row
    }

    fn without(field: &str) -> Value {
        let mut row = row();
        row.as_object_mut().unwrap().remove(field);
        row
    }

    fn read(row: Value) -> GitHubRegisteredApplication {
        serde_json::from_value(row).unwrap()
    }

    fn registered(row: Value) -> Result<RegisteredApplication, ApplicationError> {
        RegisteredApplication::try_from(read(row))
    }

    fn assert_echoes_no_credential(rendered: &str) {
        for secret in [CLIENT_SECRET, WEBHOOK_SECRET, "pem-sentinel", "BEGIN"] {
            assert!(!rendered.contains(secret), "{secret} rendered: {rendered}");
        }
    }

    fn assert_redacted(rendered: &str) {
        assert!(rendered.contains("[REDACTED]"), "{rendered}");
        assert_echoes_no_credential(rendered);
    }

    #[test]
    fn a_converted_manifest_with_extra_fields_becomes_a_registered_application() {
        let application = registered(row()).unwrap();

        let expected = RegisteredApplication::new(
            ApplicationId::new(12345),
            "octoapp",
            "Octocat App",
            Account::new("octocat-org", AccountType::Organization),
            "https://github.com/apps/octoapp",
            "Iv1.8a61f9b3a7aba766",
            SecretValue::new(CLIENT_SECRET),
            SecretValue::new(PEM),
        )
        .with_webhook_secret(SecretValue::new(WEBHOOK_SECRET));
        assert_eq!(application, expected);
    }

    #[test]
    fn a_null_or_missing_webhook_secret_is_none() {
        for row in [
            with("webhook_secret", Value::Null),
            without("webhook_secret"),
        ] {
            assert_eq!(registered(row).unwrap().webhook_secret, None);
        }
    }

    #[test]
    fn an_empty_webhook_secret_is_none() {
        let application = registered(with("webhook_secret", json!(""))).unwrap();

        assert_eq!(application.webhook_secret.expose_as_deref(), None);
    }

    #[test]
    fn a_null_or_missing_owner_is_unreadable() {
        for row in [with("owner", Value::Null), without("owner")] {
            assert!(matches!(registered(row), Err(ApplicationError::Response)));
        }
    }

    #[test]
    fn an_owner_github_would_never_put_in_a_url_is_unreadable() {
        let error = registered(with("owner", json!({ "login": "../x", "type": "User" })))
            .expect_err("an unsafe owner is refused");

        assert!(matches!(error, ApplicationError::Response));
    }

    #[test]
    fn an_empty_client_secret_or_private_key_is_unreadable_without_echoing_either() {
        for row in [with("client_secret", json!("")), with("pem", json!(""))] {
            let error = registered(row).expect_err("an empty credential is refused");

            assert!(matches!(error, ApplicationError::Response));
            assert_echoes_no_credential(&error.to_string());
        }
    }

    #[test]
    fn neither_form_renders_a_credential() {
        let wire = read(row());
        assert_redacted(&format!("{wire:?}"));

        let application = RegisteredApplication::try_from(wire).unwrap();
        assert_redacted(&format!("{application:?}"));
    }
}
