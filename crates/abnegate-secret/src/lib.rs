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
//! let key = MasterKey::generate()?;
//! let token = SecretValue::new(concat!("ghp_", "0123456789abcdefghij"));
//!
//! let stored = encrypt_value(&token, &key)?;
//! assert_eq!(decrypt_value(&stored, &key)?, token);
//!
//! assert_eq!(
//!     redact(concat!("fatal: bad token ghp_", "0123456789abcdefghij")),
//!     "fatal: bad token [REDACTED]"
//! );
//! # Ok::<(), abnegate_secret::Error>(())
//! ```
//!
//! # Features
//!
//! - `sqlx`: `Encode`, `Decode` and `Type` for [`SecretValue`] over every
//!   database that stores a `String`, so it reads and writes as the text column
//!   it is stored in.
//! - `rusqlite`: `ToSql` and `FromSql` for [`SecretValue`], the same for SQLite.
//!
//! Neither feature chooses a driver, runtime, TLS stack or SQLite build. Enable
//! those on your own `sqlx` or `rusqlite` dependency (`sqlx/postgres` and
//! `sqlx/runtime-tokio`, `rusqlite/bundled`, and so on); Cargo unifies them with
//! the bare dependency this crate declares.

mod database;
mod encryption;
mod error;
mod key;
mod random;
mod redact;
mod sanitize;
mod value;
mod work;

pub use crate::encryption::decrypt_value;
pub use crate::encryption::encrypt_value;
pub use crate::encryption::is_encrypted;
pub use crate::error::Error;
pub use crate::key::MasterKey;
pub use crate::key::default_key_path;
pub use crate::key::load_master_key;
pub use crate::key::read_key_file;
pub use crate::key::write_key_file;
pub use crate::redact::REDACTED;
pub use crate::redact::redact;
pub use crate::sanitize::sanitize;
pub use crate::sanitize::sanitize_owned;
pub use crate::value::OptionalSecretExtension;
pub use crate::value::SecretValue;
