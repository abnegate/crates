#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! The pieces a service needs to talk to the network safely.
//!
//! [`HttpClient`] is the trait a source adapter or an API client depends on, so
//! a test can stand in for the transport; [`ReqwestHttpClient`] is the
//! transport itself. [`PublicClient`], built by [`public_client`], fetches a
//! caller-supplied URL without reaching back inside the deployment: it checks
//! the URL with [`validate_public_url`] before every request, every address a
//! name resolves to, and every redirect hop. [`TargetPolicy::ExceptMetadata`]
//! narrows that guard to the [`metadata`] targets alone, for an endpoint an
//! operator or a tenant configured on loopback or the LAN, and
//! [`metadata::MetadataResolver`] guards a reqwest client of the caller's own
//! the same way. [`RateLimiter`] counts requests
//! against whatever key a caller limits by, [`Backoff`] spaces retries out, and
//! [`is_rate_limit_error`] and [`is_hard_error`] read a failure message to
//! decide which of the two a failure deserves. [`RetryAfter`] reads how long a
//! server asked to be left alone, from a `Retry-After` header or a failure
//! message, and leaves capping that wait to the caller.
//!
//! ```
//! use abnegate_http::{Backoff, Error, is_rate_limit_error, validate_public_url};
//! use std::time::Duration;
//!
//! assert!(validate_public_url("http://169.254.169.254/latest").is_err());
//! assert_eq!(
//!     validate_public_url("https://example.com/docs")?.host_str(),
//!     Some("example.com")
//! );
//!
//! assert!(is_rate_limit_error("HTTP 429 Too Many Requests"));
//!
//! assert_eq!(Backoff::default().delay_with(0, 0.0), Duration::from_secs(60));
//! # Ok::<(), Error>(())
//! ```

mod address;
mod backoff;
mod body;
mod classify;
mod client;
mod error;
mod limit;
pub mod metadata;
mod public;
mod response;
mod retry_after;
#[cfg(test)]
mod test_support;
mod transport;
mod url;

pub use crate::backoff::Backoff;
pub use crate::body::read_capped;
pub use crate::classify::is_hard_error;
pub use crate::classify::is_rate_limit_error;
pub use crate::client::HttpClient;
pub use crate::error::Error;
pub use crate::error::Result;
pub use crate::limit::Decision;
pub use crate::limit::RateLimitConfig;
pub use crate::limit::RateLimiter;
pub use crate::public::PublicClient;
pub use crate::public::PublicClientBuilder;
pub use crate::public::PublicRequest;
pub use crate::public::TargetPolicy;
pub use crate::public::public_client;
pub use crate::public::public_client_builder;
pub use crate::response::HttpResponse;
pub use crate::retry_after::RetryAfter;
pub use crate::transport::ReqwestHttpClient;
pub use crate::url::validate_public_url;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
