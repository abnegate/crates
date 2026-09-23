mod metadata;

use abnegate_secret::SecretValue;
use keyring::Entry;
use keyring::Error as KeyringError;

use crate::application::Application;
use crate::error::ConfigError;

pub use crate::token::metadata::TokenMetadata;

const ACCESS_TOKEN: &str = "access-token";
const REFRESH_TOKEN: &str = "refresh-token";
const METADATA: &str = "metadata";

/// Credentials held in the platform keyring under the application's name.
///
/// The application is the caller's: nothing here reads a configuration file
/// to find out which application it belongs to.
///
/// ```no_run
/// use abnegate_config::Application;
/// use abnegate_config::TokenMetadata;
/// use abnegate_config::TokenStore;
/// use abnegate_secret::SecretValue;
/// use chrono::TimeDelta;
/// use chrono::Utc;
///
/// let store = TokenStore::new(Application::new("example-cli")?);
/// store.set_access_token(&SecretValue::new("at-0123456789"))?;
/// store.set_metadata(&TokenMetadata::new(
///     "https://api.example.com",
///     Utc::now() + TimeDelta::hours(1),
/// ))?;
///
/// assert!(store.is_authenticated());
/// assert!(!store.metadata()?.is_expired());
/// # Ok::<(), abnegate_config::ConfigError>(())
/// ```
pub struct TokenStore {
    application: Application,
}

impl TokenStore {
    pub fn new(application: Application) -> Self {
        Self { application }
    }

    /// The keyring service every credential is stored under.
    pub fn service(&self) -> &str {
        self.application.as_str()
    }

    /// Read the credential stored under `name`.
    pub fn read(&self, name: &str) -> Result<SecretValue, ConfigError> {
        self.entry(name)?
            .get_password()
            .map(SecretValue::new)
            .map_err(|source| self.failure(name, source))
    }

    /// Store `value` under `name`, replacing whatever was there.
    pub fn write(&self, name: &str, value: &SecretValue) -> Result<(), ConfigError> {
        Ok(self.entry(name)?.set_password(value.expose())?)
    }

    /// Remove the credential stored under `name`.
    pub fn delete(&self, name: &str) -> Result<(), ConfigError> {
        self.entry(name)?
            .delete_credential()
            .map_err(|source| self.failure(name, source))
    }

    pub fn contains(&self, name: &str) -> bool {
        self.read(name).is_ok()
    }

    pub fn access_token(&self) -> Result<SecretValue, ConfigError> {
        self.read(ACCESS_TOKEN)
    }

    pub fn set_access_token(&self, token: &SecretValue) -> Result<(), ConfigError> {
        self.write(ACCESS_TOKEN, token)
    }

    pub fn refresh_token(&self) -> Result<SecretValue, ConfigError> {
        self.read(REFRESH_TOKEN)
    }

    pub fn set_refresh_token(&self, token: &SecretValue) -> Result<(), ConfigError> {
        self.write(REFRESH_TOKEN, token)
    }

    pub fn metadata(&self) -> Result<TokenMetadata, ConfigError> {
        Ok(serde_json::from_str(self.read(METADATA)?.expose())?)
    }

    pub fn set_metadata(&self, metadata: &TokenMetadata) -> Result<(), ConfigError> {
        self.write(
            METADATA,
            &SecretValue::new(serde_json::to_string(metadata)?),
        )
    }

    /// Whether an access token is stored, which is as much as this crate can
    /// say about being logged in: only the server knows if it still works.
    pub fn is_authenticated(&self) -> bool {
        self.contains(ACCESS_TOKEN)
    }

    /// Forget every credential, ignoring the ones that were never stored.
    pub fn clear(&self) -> Result<(), ConfigError> {
        for name in [ACCESS_TOKEN, REFRESH_TOKEN, METADATA] {
            match self.delete(name) {
                Ok(()) | Err(ConfigError::NoCredential { .. }) => {}
                Err(error) => return Err(error),
            }
        }

        Ok(())
    }

    fn entry(&self, name: &str) -> Result<Entry, ConfigError> {
        Ok(Entry::new(self.service(), name)?)
    }

    fn failure(&self, name: &str, source: KeyringError) -> ConfigError {
        match source {
            KeyringError::NoEntry => ConfigError::NoCredential {
                name: name.to_string(),
            },
            other => ConfigError::from(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(application: &str) -> TokenStore {
        TokenStore::new(Application::new(application).unwrap())
    }

    #[test]
    fn a_store_keeps_the_service_it_was_given() {
        assert_eq!(store("example-cli").service(), "example-cli");
    }

    #[test]
    fn two_applications_do_not_share_a_service() {
        assert_ne!(store("one").service(), store("two").service());
    }

    #[test]
    fn the_credential_names_are_stable() {
        assert_eq!(ACCESS_TOKEN, "access-token");
        assert_eq!(REFRESH_TOKEN, "refresh-token");
        assert_eq!(METADATA, "metadata");
    }

    #[test]
    fn a_missing_credential_is_not_a_store_failure() {
        let error = store("example").failure(ACCESS_TOKEN, KeyringError::NoEntry);

        assert!(
            matches!(&error, ConfigError::NoCredential { name } if name == ACCESS_TOKEN),
            "{error:?}"
        );
        assert_eq!(error.to_string(), "No credential stored for 'access-token'");
    }

    #[test]
    fn a_store_failure_is_reported_as_one() {
        let error = store("example").failure(
            ACCESS_TOKEN,
            KeyringError::Invalid("service".into(), "empty".into()),
        );

        assert!(matches!(error, ConfigError::Keyring(_)), "{error:?}");
    }

    #[test]
    fn an_undecodable_credential_is_reported_without_its_bytes() {
        let error = store("example").failure(
            ACCESS_TOKEN,
            KeyringError::BadEncoding(b"hunter2\xff".to_vec()),
        );

        assert!(
            matches!(error, ConfigError::CredentialUnreadable),
            "{error:?}"
        );
        assert!(!format!("{error:?}").contains("104"), "{error:?}");
    }
}
