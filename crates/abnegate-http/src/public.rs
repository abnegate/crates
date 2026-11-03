mod builder;
mod client;
mod request;
mod resolver;
mod target_policy;

use crate::error::Result;
use std::time::Duration;

pub use crate::public::builder::PublicClientBuilder;
pub use crate::public::client::PublicClient;
pub use crate::public::request::PublicRequest;
pub use crate::public::target_policy::TargetPolicy;

/// A builder for a [`PublicClient`], with `timeout` bounding each request.
///
/// Proxies from the environment and the operating system are switched off: a
/// proxy resolves the target itself, so the guard would only ever see the
/// proxy's name.
pub fn public_client_builder(timeout: Duration) -> PublicClientBuilder {
    PublicClientBuilder::new(timeout)
}

/// Build a [`PublicClient`] for fetching caller-supplied URLs, with `timeout`
/// bounding each request.
pub fn public_client(timeout: Duration) -> Result<PublicClient> {
    public_client_builder(timeout).build()
}
