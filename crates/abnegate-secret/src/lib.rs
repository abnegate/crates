#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! Credential handling.
//!
//! [`SecretValue`] holds a credential without letting it reach a log line by
//! accident, [`encrypt_value`] wraps one in an `ENC[v1:...]` envelope for
//! storage, [`redact`] scrubs credentials out of text on its way back to a
//! model, and [`sanitize`] does the same to text that has been through a
//! terminal.
//!
//! ```
//! use abnegate_secret::{MasterKey, SecretValue, decrypt_value, encrypt_value, redact};
//!
//! let key = MasterKey::generate();
//! let token = SecretValue::new("ghp_0123456789abcdefghij");
//!
//! let stored = encrypt_value(&token, &key)?;
//! assert_eq!(decrypt_value(&stored, &key)?, token);
//!
//! assert_eq!(
//!     redact("fatal: bad token ghp_0123456789abcdefghij"),
//!     "fatal: bad token [REDACTED]"
//! );
//! # Ok::<(), abnegate_secret::SecretError>(())
//! ```
//!
//! # Features
//!
//! - `sqlx`: `Encode`, `Decode` and `Type` for [`SecretValue`], so it reads and
//!   writes as the text column it is stored in.
//! - `rusqlite`: `ToSql` and `FromSql` for [`SecretValue`], the same for SQLite.

mod database;
mod encryption;
mod error;
mod key;
mod redact;
mod sanitize;
mod value;

pub use crate::encryption::{decrypt_value, encrypt_value, is_encrypted};
pub use crate::error::SecretError;
pub use crate::key::{MasterKey, default_key_path, load_master_key, read_key_file, write_key_file};
pub use crate::redact::{REDACTED, redact};
pub use crate::sanitize::{sanitize, sanitize_owned};
pub use crate::value::{OptionalSecretExt, SecretValue};
