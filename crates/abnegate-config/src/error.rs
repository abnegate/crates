use std::io;
use std::path::PathBuf;

use thiserror::Error;
#[cfg(feature = "keyring")]
use zeroize::Zeroize;

use crate::application::ApplicationError;

/// Every way this crate can fail to locate, read, or write configuration.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// [`Loader::load`](crate::Loader::load) found no file to read.
    #[error("Configuration file '{path}' does not exist")]
    #[non_exhaustive]
    Missing {
        /// The file that was looked for.
        path: PathBuf,
    },
    /// A configuration or `.env` file exists but could not be read.
    #[error("Failed to read '{path}'")]
    #[non_exhaustive]
    Read {
        /// The file that could not be read.
        path: PathBuf,
        /// Why reading it failed.
        #[source]
        source: io::Error,
    },
    /// A file, or the owner-only directory that holds it, could not be
    /// written.
    #[error("Failed to write '{path}'")]
    #[non_exhaustive]
    Write {
        /// The file that could not be replaced.
        path: PathBuf,
        /// Why writing it failed.
        #[source]
        source: io::Error,
    },
    /// The file is not TOML, or its TOML does not fit the settings type.
    #[error("Failed to parse '{path}'")]
    #[non_exhaustive]
    Parse {
        /// The file that could not be parsed.
        path: PathBuf,
        /// Where and why the TOML was rejected, reported against the text as
        /// written so that no decrypted value reaches the message.
        #[source]
        source: toml::de::Error,
    },
    /// The settings could not be encoded as TOML.
    #[error("Failed to serialize configuration")]
    Serialize(#[from] toml::ser::Error),
    /// The platform reports no home directory to hold the configuration.
    #[error("Home directory not found")]
    NoHomeDirectory,
    /// A name given as an [`Application`](crate::Application) is not one.
    #[error(transparent)]
    Application(#[from] ApplicationError),
    /// A key given to [`EnvironmentFile`](crate::EnvironmentFile) is not a
    /// valid environment variable name.
    #[error("'{key}' is not an environment variable name")]
    #[non_exhaustive]
    InvalidKey {
        /// The key that was refused.
        key: String,
    },
    /// A sealed value could not be opened with the loader's master key.
    #[error("Failed to decrypt '{field}'")]
    #[non_exhaustive]
    Decrypt {
        /// Where the value sits in the file, such as `database.password` or
        /// `hosts[1]`.
        field: String,
        /// Why it could not be opened:
        /// [`UnsupportedVersion`](abnegate_secret::Error::UnsupportedVersion)
        /// for an envelope this release cannot read.
        #[source]
        source: abnegate_secret::Error,
    },
    /// A value that arrived sealed could not be sealed again on save.
    #[error("Failed to encrypt '{field}'")]
    #[non_exhaustive]
    Encrypt {
        /// Where the value sits in the settings being saved.
        field: String,
        /// Why sealing it failed.
        #[source]
        source: abnegate_secret::Error,
    },
    /// A save would write a value that arrived sealed, and there is no master
    /// key to seal it again.
    #[error("'{field}' arrived sealed and there is no master key to seal it again")]
    #[non_exhaustive]
    SealedWithoutKey {
        /// Where the value sits in the settings being saved.
        field: String,
    },
    /// A value that arrived sealed can no longer be followed, because it has
    /// gone from where it was or its place now holds something other than a
    /// string, so a save refuses rather than risk writing it in the clear.
    #[error("'{field}' arrived sealed and is no longer a string that can be sealed again")]
    #[non_exhaustive]
    SealedShapeChanged {
        /// The location that lost its sealed value.
        field: String,
    },
    /// The keyring holds no credential under the name asked for.
    #[cfg(feature = "keyring")]
    #[cfg_attr(docsrs, doc(cfg(feature = "keyring")))]
    #[error("No credential stored for '{name}'")]
    #[non_exhaustive]
    NoCredential {
        /// The credential's name within the application's keyring service.
        name: String,
    },
    /// The keyring holds a credential that is not valid UTF-8 or not in the
    /// platform's format. Its bytes are zeroized and never reported.
    #[cfg(feature = "keyring")]
    #[cfg_attr(docsrs, doc(cfg(feature = "keyring")))]
    #[error("Stored credential is unreadable")]
    CredentialUnreadable,
    /// The platform credential store refused or failed the request.
    #[cfg(feature = "keyring")]
    #[cfg_attr(docsrs, doc(cfg(feature = "keyring")))]
    #[error("Credential store is unavailable")]
    Keyring(#[source] keyring::Error),
    /// Token metadata could not be encoded to, or decoded from, the JSON it is
    /// stored as.
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
            source: abnegate_secret::Error::Decryption,
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
                source: abnegate_secret::Error::Encryption,
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
