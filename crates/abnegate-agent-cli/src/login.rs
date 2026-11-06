//! Signing an agent's CLI in on someone's behalf.
//!
//! [`claude`] runs the sign-in `claude setup-token` runs: a PKCE
//! authorization link, the code claude.com shows or sends to a loopback
//! listener, and its exchange for [`claude::Tokens`] at Claude's token
//! endpoint, which [`claude::Client`] also renews them at.
//!
//! Nothing here stores a credential. Tokens come back in plaintext behind
//! [`SecretValue`](abnegate_secret::SecretValue), whose `Debug` and `Display`
//! are redacted; seal them before they reach storage. Every step fails with a
//! [`LoginError`], which never repeats a response body or anything the
//! request carried.

pub mod claude;
mod login_error;

pub use crate::login::login_error::LoginError;
