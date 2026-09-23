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

pub mod client;
pub mod config;
pub mod observe;
pub mod time_range;

pub use client::{
    SearchContext, SearchError, SearchHit, SearxngClient, build_search_url, format_search_context,
    needs_web_search, sanitize_query,
};
pub use config::{DEFAULT_SEARXNG_QUERY_URL, WebSearchConfig};
pub use observe::{SearchObserver, observe_searches};
pub use time_range::TimeRange;
