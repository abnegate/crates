use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Every way this crate can fail to seal, unseal, or persist a credential.
#[derive(Debug, Error)]
pub enum SecretError {
    #[error("Encryption failed")]
    Encryption,
    #[error("Decryption failed: invalid key or corrupted envelope")]
    Decryption,
    #[error("Invalid base64 in encrypted value")]
    InvalidBase64,
    #[error("Encrypted value is shorter than a nonce and tag")]
    Truncated,
    #[error("Decrypted value is not valid UTF-8")]
    InvalidUtf8,
    #[error("Master key is not valid hexadecimal")]
    InvalidHexadecimal,
    #[error("Master key must be {expected} bytes, got {actual}")]
    KeyLength { expected: usize, actual: usize },
    #[error("Failed to read master key file '{path}'")]
    ReadKeyFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Failed to write master key file '{path}'")]
    WriteKeyFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}
