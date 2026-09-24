use std::io;
use std::path::PathBuf;

use abnegate_secret::SecretError;
use thiserror::Error;
#[cfg(feature = "keyring")]
use zeroize::Zeroize;

use crate::application::ApplicationError;

/// Every way this crate can fail to locate, read, or write configuration.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("Configuration file '{path}' does not exist")]
    Missing { path: PathBuf },
    #[error("Failed to read '{path}'")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Failed to write '{path}'")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Failed to parse '{path}'")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("Failed to serialize configuration")]
    Serialize(#[from] toml::ser::Error),
    #[error("Home directory not found")]
    NoHomeDirectory,
    /// A name given as an [`Application`](crate::Application) is not one.
    #[error(transparent)]
    Application(#[from] ApplicationError),
    #[error("'{key}' is not an environment variable name")]
    InvalidKey { key: String },
    #[error("Failed to decrypt '{field}'")]
    Decrypt {
        field: String,
        #[source]
        source: SecretError,
    },
    #[error("Failed to encrypt '{field}'")]
    Encrypt {
        field: String,
        #[source]
        source: SecretError,
    },
    #[error("'{field}' arrived sealed and there is no master key to seal it again")]
    SealedWithoutKey { field: String },
    #[error("'{field}' arrived sealed and is no longer a string that can be sealed again")]
    SealedShapeChanged { field: String },
    #[cfg(feature = "keyring")]
    #[cfg_attr(docsrs, doc(cfg(feature = "keyring")))]
    #[error("No credential stored for '{name}'")]
    NoCredential { name: String },
    #[cfg(feature = "keyring")]
    #[cfg_attr(docsrs, doc(cfg(feature = "keyring")))]
    #[error("Stored credential is unreadable")]
    CredentialUnreadable,
    #[cfg(feature = "keyring")]
    #[cfg_attr(docsrs, doc(cfg(feature = "keyring")))]
    #[error("Credential store is unavailable")]
    Keyring(#[source] keyring::Error),
    #[cfg(feature = "keyring")]
    #[cfg_attr(docsrs, doc(cfg(feature = "keyring")))]
    #[error("Failed to encode token metadata")]
    Metadata(#[from] serde_json::Error),
}

#[cfg(feature = "keyring")]
impl From<keyring::Error> for Error {
    fn from(error: keyring::Error) -> Self {
        match error {
            keyring::Error::BadEncoding(mut credential)
            | keyring::Error::BadDataFormat(mut credential, _) => {
                credential.zeroize();
                Self::CredentialUnreadable
            }
            other => Self::Keyring(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_files_are_named() {
        let error = Error::Missing {
            path: PathBuf::from("/tmp/example/config.toml"),
        };
        assert_eq!(
            error.to_string(),
            "Configuration file '/tmp/example/config.toml' does not exist"
        );
    }

    #[test]
    fn read_failures_are_named() {
        let error = Error::Read {
            path: PathBuf::from("/tmp/example/config.toml"),
            source: io::Error::new(io::ErrorKind::NotFound, "File not found"),
        };
        assert_eq!(
            error.to_string(),
            "Failed to read '/tmp/example/config.toml'"
        );
    }

    #[test]
    fn a_missing_home_directory_is_reported() {
        assert_eq!(
            Error::NoHomeDirectory.to_string(),
            "Home directory not found"
        );
    }

    #[test]
    fn an_invalid_application_converts_with_its_own_message() {
        let invalid = ApplicationError::Invalid {
            name: "../example".to_string(),
        };

        let error = Error::from(invalid.clone());

        assert_eq!(error.to_string(), invalid.to_string());
        assert!(
            matches!(&error, Error::Application(ApplicationError::Invalid { name }) if name == "../example"),
            "{error:?}"
        );
    }

    #[test]
    fn parse_failures_carry_the_source() {
        let source = toml::from_str::<toml::Value>("invalid { toml").unwrap_err();
        let error = Error::Parse {
            path: PathBuf::from("config.toml"),
            source,
        };
        assert!(matches!(error, Error::Parse { .. }));
        assert!(error.to_string().contains("config.toml"));
    }

    #[test]
    fn serialize_failures_convert() {
        let source = toml::to_string_pretty(&toml::Value::Integer(1)).unwrap_err();
        let error: Error = source.into();
        assert!(matches!(error, Error::Serialize(_)));
    }

    #[test]
    fn decryption_failures_name_the_field() {
        let error = Error::Decrypt {
            field: "database.password".to_string(),
            source: SecretError::Decryption,
        };
        assert_eq!(error.to_string(), "Failed to decrypt 'database.password'");
    }

    #[test]
    fn a_sealed_field_without_a_key_is_named() {
        let error = Error::SealedWithoutKey {
            field: "database.password".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "'database.password' arrived sealed and there is no master key to seal it again"
        );
    }

    #[test]
    fn every_error_is_debuggable() {
        let errors = [
            Error::Missing {
                path: PathBuf::from("config.toml"),
            },
            Error::NoHomeDirectory,
            Error::Encrypt {
                field: "token".to_string(),
                source: SecretError::Encryption,
            },
            Error::InvalidKey {
                key: "1KEY".to_string(),
            },
            Error::SealedShapeChanged {
                field: "hosts[0]".to_string(),
            },
        ];

        for error in errors {
            assert!(!format!("{error:?}").is_empty());
        }
    }

    #[cfg(feature = "keyring")]
    #[test]
    fn an_undecodable_credential_never_reaches_debug_or_display() {
        const CREDENTIAL: &[u8] = b"hunter2-\xff-credential";
        let platform: Box<dyn std::error::Error + Send + Sync> = "malformed".into();

        for source in [
            keyring::Error::BadEncoding(CREDENTIAL.to_vec()),
            keyring::Error::BadDataFormat(CREDENTIAL.to_vec(), platform),
        ] {
            let error = Error::from(source);
            let rendered = format!("{error:?} {error}");

            assert!(matches!(error, Error::CredentialUnreadable), "{rendered}");
            assert!(!rendered.contains("hunter2"), "{rendered}");
            assert!(!rendered.contains("104, 117, 110"), "{rendered}");
        }
    }

    #[cfg(feature = "keyring")]
    #[test]
    fn other_store_failures_stay_wrapped() {
        let error = Error::from(keyring::Error::NoStorageAccess("locked".into()));

        assert!(
            matches!(error, Error::Keyring(keyring::Error::NoStorageAccess(_))),
            "{error:?}"
        );
    }
}
