#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! Web search through a SearXNG instance.
//!
//! [`SearxngClient`] queries one instance and returns [`SearchHit`] rows,
//! [`needs_web_search`] decides whether a message is worth a lookup at all, and
//! [`SearchContext`] turns the outcome into prompt text a model can cite.
//!
//! Search is off until it is switched on, by `SEARCH_ENABLE_WEB_SEARCH` or
//! [`WebSearchConfig::new`]. While it is off,
//! [`WebSearchConfig::requested_for`] selects no message and
//! [`SearxngClient::search`] returns [`Error::Disabled`] without sending a
//! request, so a host that asks the config first never sees that error.
//!
//! ```no_run
//! # async fn example() -> Result<(), abnegate_search::Error> {
//! use abnegate_search::{SearxngClient, TimeRange, WebSearchConfig};
//!
//! let message = "What are the latest Rust release notes?";
//! let config = WebSearchConfig::from_environment();
//! if config.requested_for(message, None) {
//!     let client = SearxngClient::new(config)?;
//!     let results = client.search(message, Some(TimeRange::Week)).await?;
//! #   let _ = results;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Untrusted results
//!
//! A result's title, URL and snippet are written by whoever published the
//! page. [`format_search_context`] sanitizes each one and keeps it to its own
//! lines inside the results block, so a page cannot close that block or pass
//! itself off as another result.

mod client;
mod config;
mod context;
mod entry;
mod error;
mod hit;
mod intent;
mod observe;
mod outcome;
mod query;
mod reply;
mod time_range;

pub use crate::client::SearxngClient;
pub use crate::config::{DEFAULT_SEARXNG_QUERY_URL, WebSearchConfig};
pub use crate::context::{SearchContext, format_search_context};
pub use crate::error::Error;
pub use crate::hit::SearchHit;
pub use crate::intent::needs_web_search;
pub use crate::observe::{SearchObserver, observe_searches};
pub use crate::outcome::Outcome;
pub use crate::query::{build_search_url, sanitize_query};
pub use crate::time_range::TimeRange;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
