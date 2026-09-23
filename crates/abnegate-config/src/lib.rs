#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! Configuration for command line applications.
//!
//! [`Config`] is the application's own settings type loaded from a TOML file,
//! [`Loader`] chooses where that file lives and which [`MasterKey`] unseals it,
//! [`config_path`] derives the conventional location from an application name,
//! [`EnvironmentFile`] upserts keys in a `.env` file, and `TokenStore` keeps
//! credentials in the platform keyring.
//!
//! [`MasterKey`]: abnegate_secret::MasterKey
//!
//! ```
//! use abnegate_config::Loader;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Default, Deserialize, Serialize)]
//! struct Settings {
//!     model: String,
//!     host: Option<String>,
//! }
//!
//! let directory = tempfile::tempdir()?;
//! let loader = Loader::at(directory.path().join("config.toml"));
//!
//! let mut config = loader.load_or_default::<Settings>()?;
//! config.value_mut().model = "gpt-4o".to_string();
//! config.save()?;
//!
//! assert!(loader.exists());
//! assert_eq!(loader.load::<Settings>()?.value().model, "gpt-4o");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Sealed values
//!
//! Any string in the file written as an `ENC[v1:...]` envelope is decrypted on
//! load when the loader is given a master key, so an application reads a
//! password as a password and never learns that it was encrypted at rest.
//! [`Config::save_sealed`] writes those same fields back sealed.
//!
//! ```
//! use abnegate_config::Loader;
//! use abnegate_secret::{MasterKey, SecretValue, encrypt_value};
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Deserialize, Serialize)]
//! struct Settings {
//!     password: String,
//! }
//!
//! let key = MasterKey::generate();
//! let directory = tempfile::tempdir()?;
//! let path = directory.path().join("config.toml");
//! std::fs::write(
//!     &path,
//!     format!(
//!         "password = \"{}\"\n",
//!         encrypt_value(&SecretValue::new("hunter2"), &key)?
//!     ),
//! )?;
//!
//! let config = Loader::at(&path).master_key(&key).load::<Settings>()?;
//! assert_eq!(config.value().password, "hunter2");
//!
//! config.save_sealed(&key)?;
//! assert!(std::fs::read_to_string(&path)?.contains("ENC[v1:"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Features
//!
//! - `keyring`: `TokenStore` and `TokenMetadata`, a credential store backed
//!   by Keychain Services, the Windows Credential Manager, or the Secret
//!   Service.

mod config;
mod envelope;
mod environment;
mod error;
mod loader;
mod path;
#[cfg(feature = "keyring")]
mod token;

pub use crate::config::Config;
pub use crate::environment::EnvironmentFile;
pub use crate::error::ConfigError;
pub use crate::loader::Loader;
pub use crate::path::{config_dir, config_path};
#[cfg(feature = "keyring")]
#[cfg_attr(docsrs, doc(cfg(feature = "keyring")))]
pub use crate::token::{TokenMetadata, TokenStore};
