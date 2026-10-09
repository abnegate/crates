//! The operations a host needs, behind a trait so a test can stand in.

use async_trait::async_trait;

use crate::checkout_request::CheckoutRequest;
use crate::error::Result;
use crate::portal_request::PortalRequest;
use crate::session::Session;

/// Create Checkout and Customer Portal sessions.
///
/// [`Client`](crate::Client) is the Stripe implementation. The `testing`
/// feature provides `Fake`, which records instead.
#[async_trait]
pub trait Payments: Send + Sync {
    /// Open a Checkout Session and return the hosted URL.
    async fn checkout(&self, request: CheckoutRequest) -> Result<Session>;

    /// Open a Customer Portal session and return the hosted URL.
    async fn portal(&self, request: PortalRequest) -> Result<Session>;
}
