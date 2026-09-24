#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! Configuration for command line applications.
//!
//! [`Config`] is the application's own settings type loaded from a TOML file,
//! [`Loader`] chooses where that file lives and which [`MasterKey`] unseals it,
//! [`path`](fn@path) and [`directory`] derive the conventional locations from
//! an [`Application`] name, [`EnvironmentFile`] upserts keys in a `.env` file,
//! and `TokenStore` keeps credentials in the platform keyring.
//!
//! Every file this crate writes is replaced atomically and is readable only by
//! its owner, as is any directory it creates to hold one.
//!
//! [`MasterKey`]: abnegate_secret::MasterKey
//!
//! ```
//! use abnegate_config::Loader;
//! use serde::Deserialize;
//! use serde::Serialize;
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
//! [`Config::save`] seals those same values again wherever they now appear,
//! under a renamed key or at a position an array shifted them to by losing
//! other elements, and refuses rather than write one in the clear: with
//! [`Error::SealedWithoutKey`] when there is no key to seal it with, and
//! with [`Error::SealedShapeChanged`] when a sealed value has gone and
//! cannot be told apart from one that was renamed and edited.
//!
//! A value counts as moved only while at least as many strings hold it as the
//! file did, so a copy under another key does not vouch for it: renaming a
//! secret that two fields shared and editing one of them is refused. An empty
//! value is followed only by where it sat, never by content, since any empty
//! or defaulted string would match it, so one whose key is gone is refused.
//!
//! When an array on a sealed value's key path grows, is reordered, or has a
//! string edited, the value may be any string on that key path, so every one
//! of them is sealed, plain neighbours included. A neighbour sealed needlessly
//! reads back as it was on the next load; a secret written in the clear
//! cannot be taken back. Without a key such a save is refused instead.
//!
//! ```
//! use abnegate_config::Loader;
//! use abnegate_secret::MasterKey;
//! use abnegate_secret::SecretValue;
//! use abnegate_secret::encrypt_value;
//! use serde::Deserialize;
//! use serde::Serialize;
//!
//! #[derive(Deserialize, Serialize)]
//! struct Settings {
//!     model: String,
//!     password: String,
//! }
//!
//! let key = MasterKey::generate()?;
//! let directory = tempfile::tempdir()?;
//! let path = directory.path().join("config.toml");
//! std::fs::write(
//!     &path,
//!     format!(
//!         "model = \"gpt-4o\"\npassword = \"{}\"\n",
//!         encrypt_value(&SecretValue::new("hunter2"), &key)?
//!     ),
//! )?;
//!
//! let mut config = Loader::at(&path).master_key(&key).load::<Settings>()?;
//! assert_eq!(config.value().password, "hunter2");
//!
//! config.value_mut().model = "gpt-5".to_string();
//! config.save()?;
//! let written = std::fs::read_to_string(&path)?;
//! assert!(written.contains("ENC[v1:"));
//! assert!(!written.contains("hunter2"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Features
//!
//! - `keyring`: `TokenStore` and `TokenMetadata`, a credential store backed
//!   by Keychain Services, the Windows Credential Manager, or the Secret
//!   Service.

mod application;
mod config;
mod envelope;
mod environment;
mod error;
mod loader;
mod path;
mod private_file;
#[cfg(feature = "keyring")]
mod token;

pub use crate::application::Application;
pub use crate::application::ApplicationError;
pub use crate::application::DEFAULT_APPLICATION;
pub use crate::config::Config;
pub use crate::environment::EnvironmentFile;
pub use crate::error::Error;
pub use crate::loader::Loader;
pub use crate::path::directory;
pub use crate::path::path;
#[cfg(feature = "keyring")]
#[cfg_attr(docsrs, doc(cfg(feature = "keyring")))]
pub use crate::token::TokenMetadata;
#[cfg(feature = "keyring")]
#[cfg_attr(docsrs, doc(cfg(feature = "keyring")))]
pub use crate::token::TokenStore;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
