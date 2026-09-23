#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! The pieces a service needs to talk to the network safely.
//!
//! [`HttpClient`] is the trait a source adapter or an API client depends on, so
//! a test can stand in for the transport; [`ReqwestHttpClient`] is the
//! transport itself. [`PublicClient`], built by [`public_client`], fetches a
//! caller-supplied URL without reaching back inside the deployment: it checks
//! the URL with [`validate_public_url`] before every request, every address a
//! name resolves to, and every redirect hop. [`RateLimiter`] counts requests
//! against whatever key a caller limits by, [`Backoff`] spaces retries out, and
//! [`is_rate_limit_error`] and [`is_hard_error`] read a failure message to
//! decide which of the two a failure deserves.
//!
//! ```
//! use abnegate_http::{Backoff, HttpError, is_rate_limit_error, validate_public_url};
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
//! # Ok::<(), HttpError>(())
//! ```

mod address;
mod backoff;
mod body;
mod classify;
mod client;
mod error;
mod limit;
mod public;
mod response;
#[cfg(test)]
mod test_support;
mod transport;
mod url;

pub use crate::backoff::Backoff;
pub use crate::body::read_capped;
pub use crate::classify::is_hard_error;
pub use crate::classify::is_rate_limit_error;
pub use crate::client::HttpClient;
pub use crate::error::HttpError;
pub use crate::error::Result;
pub use crate::limit::Decision;
pub use crate::limit::RateLimitConfig;
pub use crate::limit::RateLimiter;
pub use crate::public::PublicClient;
pub use crate::public::PublicClientBuilder;
pub use crate::public::public_client;
pub use crate::public::public_client_builder;
pub use crate::response::HttpResponse;
pub use crate::transport::ReqwestHttpClient;
pub use crate::url::validate_public_url;
