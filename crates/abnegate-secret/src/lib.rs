#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! Credential handling.
//!
//! [`SecretValue`] holds a credential without letting it reach a log line by
//! accident, `encrypt_value` wraps one in an `ENC[v1:...]` envelope for
//! storage, [`redact`] scrubs credentials out of text on its way back to a
//! model, [`contains_credential`] tells whether a value meant to be a plain
//! name carries one, [`conceal`] removes one known key however a provider
//! echoes it, and [`sanitize`] scrubs text that has been through a terminal.
//!
//! ```
//! use abnegate_secret::redact;
//!
//! assert_eq!(
//!     redact(concat!("fatal: bad token ghp_", "0123456789abcdefghij")),
//!     "fatal: bad token [REDACTED]"
//! );
//! ```
//!
//! ```
//! # #[cfg(feature = "encryption")]
//! # fn main() -> Result<(), abnegate_secret::Error> {
//! use abnegate_secret::{MasterKey, SecretValue, decrypt_value, encrypt_value};
//!
//! let key = MasterKey::generate()?;
//! let token = SecretValue::new(concat!("ghp_", "0123456789abcdefghij"));
//!
//! let stored = encrypt_value(&token, &key)?;
//! assert_eq!(decrypt_value(&stored, &key)?, token);
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "encryption"))]
//! # fn main() {}
//! ```
//!
//! # Features
//!
//! - `encryption`, on by default: `encrypt_value`, `decrypt_value`,
//!   `decrypt_legacy_value` for values sealed before the envelope existed, and
//!   the `MasterKey` they seal under, with the AES-256-GCM, random source and
//!   key file dependencies they need. Turn default features off to only redact.
//! - `derivation`: `MasterKey::derive`, which stretches a passphrase into a key
//!   with Argon2id, and the `KeyDerivation` that names its domain. Implies
//!   `encryption`.
//! - `sqlx`: `Encode`, `Decode` and `Type` for [`SecretValue`] over every
//!   database that stores a `String`, so it reads and writes as the text column
//!   it is stored in.
//!
//! The `sqlx` feature implements sqlx 0.9's traits and chooses no driver,
//! runtime or TLS stack. Enable those on your own `sqlx` 0.9 dependency
//! (`sqlx/postgres`, `sqlx/runtime-tokio` and so on); Cargo unifies them with
//! the bare dependency this crate declares. Another sqlx minor is a separate
//! crate and does not see these implementations.

mod conceal;
mod database;
#[cfg(feature = "encryption")]
mod encryption;
#[cfg(feature = "encryption")]
mod error;
#[cfg(feature = "encryption")]
mod key;
mod optional;
#[cfg(feature = "encryption")]
mod random;
mod redact;
mod sanitize;
mod value;
mod work;

pub use crate::conceal::conceal;
#[cfg(feature = "encryption")]
pub use crate::encryption::decrypt_legacy_value;
#[cfg(feature = "encryption")]
pub use crate::encryption::decrypt_value;
#[cfg(feature = "encryption")]
pub use crate::encryption::encrypt_value;
#[cfg(feature = "encryption")]
pub use crate::encryption::is_encrypted;
#[cfg(feature = "encryption")]
pub use crate::error::Error;
#[cfg(feature = "derivation")]
pub use crate::key::KeyDerivation;
#[cfg(feature = "encryption")]
pub use crate::key::MasterKey;
#[cfg(feature = "encryption")]
pub use crate::key::default_key_path;
#[cfg(feature = "encryption")]
pub use crate::key::load_master_key;
#[cfg(feature = "encryption")]
pub use crate::key::read_key_file;
#[cfg(feature = "encryption")]
pub use crate::key::write_key_file;
pub use crate::optional::OptionalSecretExtension;
pub use crate::redact::REDACTED;
pub use crate::redact::contains_credential;
pub use crate::redact::redact;
pub use crate::sanitize::sanitize;
pub use crate::sanitize::sanitize_owned;
pub use crate::value::SecretValue;

#[cfg(all(doctest, feature = "encryption"))]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
