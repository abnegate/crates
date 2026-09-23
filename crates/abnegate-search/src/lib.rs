#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! Web search through a SearXNG instance.
//!
//! [`SearxngClient`] queries one instance and returns [`SearchHit`] rows,
//! [`needs_web_search`] decides whether a message is worth a lookup at all, and
//! [`SearchContext`] turns the outcome into prompt text a model can cite.
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use abnegate_search::{SearxngClient, TimeRange, WebSearchConfig};
//!
//! let client = SearxngClient::new(WebSearchConfig::from_env())?;
//! let results = client
//!     .search("rust release notes", Some(TimeRange::Week))
//!     .await?;
//! # let _ = results;
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
pub use crate::error::SearchError;
pub use crate::hit::SearchHit;
pub use crate::intent::needs_web_search;
pub use crate::observe::{SearchObserver, observe_searches};
pub use crate::outcome::Outcome;
pub use crate::query::{build_search_url, sanitize_query};
pub use crate::time_range::TimeRange;
