#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! The pieces a service needs to talk to the network safely.
//!
//! [`HttpClient`] is the trait a source adapter or an API client depends on, so
//! a test can stand in for the transport; [`ReqwestHttpClient`] is the
//! transport itself. [`validate_public_url`] and [`public_client`] guard a
//! fetch of a caller-supplied URL against reaching back inside the deployment,
//! at the URL and again at every address it resolves to.
//!
//! ```
//! use abnegate_http::{HttpError, validate_public_url};
//!
//! assert!(validate_public_url("http://169.254.169.254/latest").is_err());
//! assert_eq!(
//!     validate_public_url("https://example.com/docs")?.host_str(),
//!     Some("example.com")
//! );
//! # Ok::<(), HttpError>(())
//! ```

mod client;
mod error;
mod response;
mod transport;
mod url;

pub use crate::client::HttpClient;
pub use crate::error::{HttpError, Result};
pub use crate::response::HttpResponse;
pub use crate::transport::ReqwestHttpClient;
pub use crate::url::{public_client, public_client_builder, read_capped, validate_public_url};
