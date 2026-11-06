use crate::pull_request::application::Account;
use crate::pull_request::application::ApplicationId;
use abnegate_secret::SecretValue;

/// A GitHub App that a manifest registered, with the credentials GitHub
/// hands over once, when the manifest's code is converted.
///
/// The client secret, the webhook secret and the private key are each a
/// [`SecretValue`], so `Debug` renders them as `[REDACTED]`. GitHub never
/// shows the private key or the client secret again, so they must be stored
/// before this is dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RegisteredApplication {
    /// GitHub's identifier for the App, the issuer of every JWT it signs.
    pub id: ApplicationId,
    /// The App's slug, which names it in URLs.
    pub slug: String,
    /// The App's display name.
    pub name: String,
    /// The account that owns the App.
    pub owner: Account,
    /// Where the App's public page can be read in a browser.
    pub html_url: String,
    /// The App's OAuth client identifier.
    pub client_id: String,
    /// The App's OAuth client secret.
    pub client_secret: SecretValue,
    /// The secret GitHub signs the App's webhook deliveries with, or `None`
    /// when the manifest asked for none.
    pub webhook_secret: Option<SecretValue>,
    /// The App's RSA private key in PEM form, which signs its JWTs with
    /// [`crate::pull_request::ApplicationClaims::sign`].
    pub private_key: SecretValue,
}

impl RegisteredApplication {
    /// The App GitHub identifies by `id`, named `name` and `slug`, owned by
    /// `owner` and shown at `html_url`, with the OAuth client `client_id` and
    /// `client_secret`, signing its JWTs with `private_key`, and with no
    /// webhook secret.
    #[expect(
        clippy::too_many_arguments,
        reason = "every argument is a field GitHub always sends"
    )]
    pub fn new(
        id: ApplicationId,
        slug: impl Into<String>,
        name: impl Into<String>,
        owner: Account,
        html_url: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: SecretValue,
        private_key: SecretValue,
    ) -> Self {
        Self {
            id,
            slug: slug.into(),
            name: name.into(),
            owner,
            html_url: html_url.into(),
            client_id: client_id.into(),
            client_secret,
            webhook_secret: None,
            private_key,
        }
    }

    /// The secret GitHub signs the App's webhook deliveries with.
    #[must_use]
    pub fn with_webhook_secret(mut self, webhook_secret: SecretValue) -> Self {
        self.webhook_secret = Some(webhook_secret);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::application::AccountType;
    use abnegate_secret::OptionalSecretExtension;

    fn registered() -> RegisteredApplication {
        RegisteredApplication::new(
            ApplicationId::new(12345),
            "octoapp",
            "Octocat App",
            Account::new("octocat-org", AccountType::Organization),
            "https://github.com/apps/octoapp",
            "Iv1.8a61f9b3a7aba766",
            SecretValue::new("client-secret-sentinel"),
            SecretValue::new("private-key-sentinel"),
        )
    }

    #[test]
    fn a_new_registered_application_holds_every_field_and_no_webhook_secret() {
        let application = registered();

        assert_eq!(application.id, ApplicationId::new(12345));
        assert_eq!(application.slug, "octoapp");
        assert_eq!(application.name, "Octocat App");
        assert_eq!(
            application.owner,
            Account::new("octocat-org", AccountType::Organization)
        );
        assert_eq!(application.html_url, "https://github.com/apps/octoapp");
        assert_eq!(application.client_id, "Iv1.8a61f9b3a7aba766");
        assert_eq!(application.client_secret.expose(), "client-secret-sentinel");
        assert_eq!(application.private_key.expose(), "private-key-sentinel");
        assert_eq!(application.webhook_secret, None);
    }

    #[test]
    fn with_webhook_secret_sets_the_webhook_secret_and_nothing_else() {
        let application = registered().with_webhook_secret(SecretValue::new("webhook-sentinel"));

        assert_eq!(
            application.webhook_secret.expose_as_deref(),
            Some("webhook-sentinel")
        );
        assert_eq!(
            RegisteredApplication {
                webhook_secret: None,
                ..application
            },
            registered()
        );
    }
}
