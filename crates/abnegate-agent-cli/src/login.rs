//! Signing an agent's CLI in on someone's behalf, and asking it whether it
//! is signed in.
//!
//! [`claude`] runs the sign-in `claude setup-token` runs: a PKCE
//! authorization link, the code claude.com shows or sends to a loopback
//! listener, and its exchange for [`claude::Tokens`] at Claude's token
//! endpoint, which [`claude::Client`] also renews them at.
//!
//! [`codex`] runs `codex login --device-auth` in a private staging directory
//! and hands back the link and one-time code it prints, then the login it
//! saved, for the caller to promote into a home or keep however it keeps
//! credentials. [`Status::check`] asks either CLI whether it is signed in.
//! Every CLI runs with exactly the environment it is given, in a process
//! group of its own that is stopped with it.
//!
//! Nothing here stores a credential. Tokens come back in plaintext behind
//! [`SecretValue`](abnegate_secret::SecretValue), whose `Debug` and `Display`
//! are redacted; seal them before they reach storage. Every step fails with a
//! [`LoginError`], which never repeats a response body or anything the
//! request carried.

pub mod claude;
pub mod codex;
mod login_error;
mod output;
mod process;
mod status;

pub use crate::login::login_error::LoginError;
pub use crate::login::status::Status;
