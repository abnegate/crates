#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! The pieces a service needs to talk to the network safely.
//!
//! [`HttpClient`] is the trait a source adapter or an API client depends on, so
//! a test can stand in for the transport; [`ReqwestHttpClient`] is the
//! transport itself.

mod client;
mod error;
mod response;
mod transport;

pub use crate::client::HttpClient;
pub use crate::error::{HttpError, Result};
pub use crate::response::HttpResponse;
pub use crate::transport::ReqwestHttpClient;
