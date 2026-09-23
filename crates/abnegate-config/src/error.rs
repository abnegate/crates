use std::io;
use std::path::PathBuf;

use abnegate_secret::SecretError;
use thiserror::Error;
#[cfg(feature = "keyring")]
use zeroize::Zeroize;

/// Every way this crate can fail to locate, read, or write configuration.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
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
impl From<keyring::Error> for ConfigError {
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
        let error = ConfigError::Missing {
            path: PathBuf::from("/tmp/example/config.toml"),
        };
        assert_eq!(
            error.to_string(),
            "Configuration file '/tmp/example/config.toml' does not exist"
        );
    }

    #[test]
    fn read_failures_are_named() {
        let error = ConfigError::Read {
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
            ConfigError::NoHomeDirectory.to_string(),
            "Home directory not found"
        );
    }

    #[test]
    fn parse_failures_carry_the_source() {
        let source = toml::from_str::<toml::Value>("invalid { toml").unwrap_err();
        let error = ConfigError::Parse {
            path: PathBuf::from("config.toml"),
            source,
        };
        assert!(matches!(error, ConfigError::Parse { .. }));
        assert!(error.to_string().contains("config.toml"));
    }

    #[test]
    fn serialize_failures_convert() {
        let source = toml::to_string_pretty(&toml::Value::Integer(1)).unwrap_err();
        let error: ConfigError = source.into();
        assert!(matches!(error, ConfigError::Serialize(_)));
    }

    #[test]
    fn decryption_failures_name_the_field() {
        let error = ConfigError::Decrypt {
            field: "database.password".to_string(),
            source: SecretError::Decryption,
        };
        assert_eq!(error.to_string(), "Failed to decrypt 'database.password'");
    }

    #[test]
    fn every_error_is_debuggable() {
        let errors = [
            ConfigError::Missing {
                path: PathBuf::from("config.toml"),
            },
            ConfigError::NoHomeDirectory,
            ConfigError::Encrypt {
                field: "token".to_string(),
                source: SecretError::Encryption,
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
            let error = ConfigError::from(source);
            let rendered = format!("{error:?} {error}");

            assert!(
                matches!(error, ConfigError::CredentialUnreadable),
                "{rendered}"
            );
            assert!(!rendered.contains("hunter2"), "{rendered}");
            assert!(!rendered.contains("104, 117, 110"), "{rendered}");
        }
    }

    #[cfg(feature = "keyring")]
    #[test]
    fn other_store_failures_stay_wrapped() {
        let error = ConfigError::from(keyring::Error::NoStorageAccess("locked".into()));

        assert!(
            matches!(
                error,
                ConfigError::Keyring(keyring::Error::NoStorageAccess(_))
            ),
            "{error:?}"
        );
    }
}
