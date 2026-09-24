use std::fmt;

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use crate::redact::REDACTED;

/// A credential string.
///
/// `Debug` and `Display` print [`REDACTED`], the buffer is zeroized on drop,
/// and comparison is constant time. Read the value with
/// [`SecretValue::expose`] at the point of use.
///
/// ```
/// use abnegate_secret::SecretValue;
///
/// let token = SecretValue::new(concat!("ghp_", "0123456789abcdefghij"));
/// assert_eq!(format!("{token:?}"), "[REDACTED]");
/// assert_eq!(token.expose(), concat!("ghp_", "0123456789abcdefghij"));
/// ```
///
/// # Serialization
///
/// `Serialize` writes the credential **in plaintext**, and `Deserialize` reads
/// plaintext back, so a settings file that holds one round-trips. Redaction
/// covers `Debug` and `Display` only: seal the value with
/// [`encrypt_value`](crate::encrypt_value) before it reaches storage, and
/// never serialize a `SecretValue` into a log line, an error report or a
/// response body.
///
/// ```
/// use abnegate_secret::SecretValue;
///
/// let token = SecretValue::new(concat!("ghp_", "0123456789abcdefghij"));
/// assert_eq!(serde_json::to_string(&token)?, concat!("\"ghp_", "0123456789abcdefghij\""));
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone)]
pub struct SecretValue {
    inner: String,
}

impl SecretValue {
    /// Hold `value` as a credential.
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            inner: value.into(),
        }
    }

    /// Read the credential. Call this at the point of use and nowhere else.
    pub fn expose(&self) -> &str {
        &self.inner
    }

    /// Whether the credential is empty, without exposing it.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    #[cfg(feature = "sqlx")]
    pub(crate) fn expose_buffer(&self) -> &String {
        &self.inner
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.inner.zeroize();
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(REDACTED)
    }
}

impl fmt::Display for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(REDACTED)
    }
}

impl PartialEq for SecretValue {
    fn eq(&self, other: &Self) -> bool {
        self.inner.as_bytes().ct_eq(other.inner.as_bytes()).into()
    }
}

impl Eq for SecretValue {}

impl From<String> for SecretValue {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for SecretValue {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// Writes the exposed credential in plaintext; see
/// [Serialization](SecretValue#serialization).
impl Serialize for SecretValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.inner)
    }
}

impl<'de> Deserialize<'de> for SecretValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::new)
    }
}

/// `Option<SecretValue>` counterparts to the `Option<String>` methods.
pub trait OptionalSecretExtension {
    /// The exposed credential, as [`Option::as_deref`] would give it.
    fn expose_as_deref(&self) -> Option<&str>;
}

impl OptionalSecretExtension for Option<SecretValue> {
    fn expose_as_deref(&self) -> Option<&str> {
        self.as_ref().map(SecretValue::expose)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize)]
    struct Settings {
        token: SecretValue,
    }

    #[test]
    fn display_is_redacted_so_it_must_never_reach_an_authorization_header() {
        let secret = SecretValue::new(concat!("ghp_", "a_real_looking_token_value"));
        assert_eq!(secret.to_string(), REDACTED);
        assert_ne!(
            secret.to_string(),
            secret.expose(),
            "a Display-taking API such as bearer_auth needs expose(), not the SecretValue"
        );
    }

    #[test]
    fn debug_is_redacted() {
        let secret = SecretValue::new(concat!("ghp_", "notarealtokenatall"));
        assert_eq!(format!("{secret:?}"), REDACTED);
        assert!(!format!("{secret:?}").contains("ghp_"));
    }

    #[test]
    fn display_is_redacted() {
        let secret = SecretValue::new(concat!("ghp_", "notarealtokenatall"));
        assert_eq!(secret.to_string(), REDACTED);
    }

    #[test]
    fn debug_of_containing_struct_is_redacted() {
        #[derive(Debug)]
        struct Row {
            key: Option<SecretValue>,
        }

        let row = Row {
            key: Some(SecretValue::new("sk-live-secret-value")),
        };
        assert_eq!(row.key.expose_as_deref(), Some("sk-live-secret-value"));

        let rendered = format!("{row:?}");
        assert_eq!(rendered, "Row { key: Some([REDACTED]) }");
        assert!(!rendered.contains("sk-live"));
    }

    #[test]
    fn expose_returns_the_value() {
        assert_eq!(SecretValue::new("value").expose(), "value");
    }

    #[test]
    fn is_empty_tracks_the_value() {
        assert!(SecretValue::new("").is_empty());
        assert!(!SecretValue::new("value").is_empty());
    }

    #[test]
    fn clone_keeps_the_value() {
        let secret = SecretValue::new("value");
        assert_eq!(secret.clone().expose(), "value");
    }

    #[test]
    fn equality_is_by_value() {
        assert_eq!(SecretValue::new("value"), SecretValue::new("value"));
        assert_ne!(SecretValue::new("value"), SecretValue::new("other"));
        assert_ne!(SecretValue::new("value"), SecretValue::new("value-longer"));
    }

    #[test]
    fn serde_round_trips_as_a_plain_string() {
        let settings: Settings = serde_json::from_str(r#"{"token":"sk-plain-value"}"#).unwrap();
        assert_eq!(settings.token.expose(), "sk-plain-value");

        let encoded = serde_json::to_string(&settings).unwrap();
        assert_eq!(encoded, r#"{"token":"sk-plain-value"}"#);
    }

    #[test]
    fn serde_round_trips_through_toml() {
        let settings: Settings = toml::from_str(r#"token = "my-token""#).unwrap();
        assert_eq!(settings.token.expose(), "my-token");

        let encoded = toml::to_string(&settings).unwrap();
        assert!(encoded.contains("my-token"));
    }

    #[test]
    fn expose_as_deref_mirrors_option_as_deref() {
        let present: Option<SecretValue> = Some(SecretValue::new("value"));
        let absent: Option<SecretValue> = None;
        assert_eq!(present.expose_as_deref(), Some("value"));
        assert_eq!(absent.expose_as_deref(), None);
    }
}
