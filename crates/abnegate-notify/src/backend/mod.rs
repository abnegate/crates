//! The channels this crate ships with.
//!
//! Each backend is one file implementing [`Notifier`](crate::Notifier).
//! Adding another is the same: a file here, exported below.

mod discord;
#[cfg(feature = "smtp")]
mod email;
mod slack;
mod webhook;

pub use crate::backend::discord::Discord;
#[cfg(feature = "smtp")]
pub use crate::backend::email::Email;
pub use crate::backend::slack::Slack;
