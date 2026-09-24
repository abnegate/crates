#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! One notification, every channel.
//!
//! [`Fanout`] delivers a [`Notification`] to every registered [`Notifier`]
//! concurrently and returns a [`Report`] saying what each one did. A channel
//! that hangs, fails or panics costs one entry in that report and nothing
//! else, so a misconfigured webhook cannot stop an email going out.
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use abnegate_notify::{Discord, Fanout, Notification, Severity, Slack};
//!
//! let fanout = Fanout::new()
//!     .with(Slack::new("https://hooks.slack.com/services/T000/B000/xxxx")?)
//!     .with(Discord::new("https://discord.com/api/webhooks/1/xxxx")?);
//!
//! let report = fanout
//!     .deliver(
//!         &Notification::new("Build failed", "3 tests failed on main")
//!             .severity(Severity::Error)
//!             .link("https://example.test/builds/1"),
//!     )
//!     .await;
//!
//! for failure in report.failures() {
//!     eprintln!("{} did not take it: {:?}", failure.name(), failure.error());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Features
//!
//! - `smtp`: the `Email` channel, and `Mailer`, which sends one message through
//!   a relay. Both pull in `lettre`. Slack, Discord and the shared webhook
//!   client are always available.
//! - `mock`: `MockMailer`, a [`Mail`] that records instead of sending, for a
//!   caller's own tests.
//!
//! # Credentials
//!
//! A webhook URL is a bearer credential: anyone holding it can post to the
//! channel. [`Endpoint`] keeps one in a
//! [`SecretValue`](abnegate_secret::SecretValue) and no error, `Debug` or log
//! line in this crate reproduces it. That includes errors from `reqwest`,
//! whose own `Display` appends the request URL and is stripped before it is
//! ever rendered.
//!
//! # Content
//!
//! A notification's text passes through [`abnegate_secret::sanitize`] as it is
//! built, which strips terminal control sequences and redacts credentials.
//! Bodies carry tool output and user data, a chat channel's history is much
//! harder to scrub than a log file, and control sequences in a message can
//! forge output in a terminal-based reader.
//!
//! # Outbound requests
//!
//! A configured webhook URL points wherever its author said, which makes
//! delivery a server-side request forgery surface. Each backend accepts only
//! `https` URLs whose host matches its provider exactly, and redirects are
//! refused rather than followed.

mod backend;
mod channel;
mod delivery;
mod endpoint;
mod error;
mod fanout;
mod field;
mod mail;
mod notification;
mod notifier;
mod report;
mod severity;
mod smtp;
mod text;

#[cfg(feature = "smtp")]
pub use crate::backend::Email;
pub use crate::backend::{Discord, Slack};
pub use crate::channel::Channel;
pub use crate::delivery::Delivery;
pub use crate::endpoint::{Endpoint, EndpointError};
pub use crate::error::Error;
pub use crate::fanout::{DEFAULT_TIMEOUT, Fanout};
pub use crate::field::Field;
pub use crate::mail::Mail;
#[cfg(feature = "smtp")]
pub use crate::mail::Mailer;
#[cfg(feature = "mock")]
pub use crate::mail::{MockMailer, SentMail};
pub use crate::notification::Notification;
pub use crate::notifier::Notifier;
pub use crate::report::Report;
pub use crate::severity::Severity;
pub use crate::smtp::SmtpConfig;
