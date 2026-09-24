use std::io;
use std::path::PathBuf;

/// Every way this crate can fail to seal, unseal, or persist a credential.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The cipher could not seal the value.
    #[error("Encryption failed")]
    Encryption,
    /// The envelope did not open under the key: the key is wrong, or the
    /// envelope was altered.
    #[error("Decryption failed: invalid key or corrupted envelope")]
    Decryption,
    /// The envelope's body is not base64.
    #[error("Invalid base64 in encrypted value")]
    InvalidBase64,
    /// The envelope is never closed, or its body is too short to hold a nonce
    /// and a tag.
    #[error("Encrypted value is truncated")]
    Truncated,
    /// The value is sealed in an envelope version this release cannot open.
    ///
    /// It is still sealed, never plaintext: store it back as it was read.
    #[error("Encrypted value uses envelope version {version}, which this release cannot open")]
    UnsupportedVersion {
        /// The digits after `ENC[v`, as written.
        version: String,
    },
    /// The envelope opened, but what it held is not UTF-8 text.
    #[error("Decrypted value is not valid UTF-8")]
    InvalidUtf8,
    /// The operating system could not supply random bytes for a key, a nonce
    /// or a temporary file name.
    #[error("The operating system could not supply random bytes")]
    Entropy {
        /// Why the random source failed.
        #[source]
        source: io::Error,
    },
    /// A master key is not hexadecimal.
    #[error("Master key is not valid hexadecimal")]
    InvalidHexadecimal,
    /// A master key decoded to the wrong number of bytes.
    #[error("Master key must be {expected} bytes, got {actual}")]
    KeyLength {
        /// The bytes an AES-256 key takes.
        expected: usize,
        /// The bytes the key decoded to.
        actual: usize,
    },
    /// A master key variable holds something other than UTF-8.
    #[error("Environment variable '{variable}' is not valid UTF-8")]
    InvalidEnvironment {
        /// The variable's name.
        variable: String,
    },
    /// There is no home directory to look for the default key file under.
    #[error("No home directory to look for the master key under")]
    NoHomeDirectory,
    /// A master key file could not be read.
    #[error("Failed to read master key file '{path}'")]
    ReadKeyFile {
        /// The file.
        path: PathBuf,
        /// Why it could not be read.
        #[source]
        source: io::Error,
    },
    /// A master key file could not be written.
    #[error("Failed to write master key file '{path}'")]
    WriteKeyFile {
        /// The file.
        path: PathBuf,
        /// Why it could not be written.
        #[source]
        source: io::Error,
    },
}
