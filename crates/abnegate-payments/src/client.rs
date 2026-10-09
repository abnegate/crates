//! The Stripe implementation of [`Payments`].

use abnegate_secret::SecretValue;
use async_trait::async_trait;
use stripe::ClientBuilder;
use stripe_billing::billing_portal_session::CreateBillingPortalSession;
use stripe_checkout::checkout_session::CreateCheckoutSession;
use stripe_checkout::checkout_session::CreateCheckoutSessionLineItems;

use crate::checkout_request::CheckoutRequest;
use crate::error::Error;
use crate::error::Result;
use crate::payments::Payments;
use crate::portal_request::PortalRequest;
use crate::session::Session;

/// A Stripe client that opens Checkout and Customer Portal sessions.
///
/// Built from a [`SecretValue`](SecretValue) through [`ClientBuilder`], which
/// is the fallible path. [`stripe::Client::new`] panics if the secret is not a
/// valid header value; this constructor never does.
#[derive(Clone, Debug)]
pub struct Client {
    inner: stripe::Client,
}

impl Client {
    /// A client that authenticates with `secret`.
    ///
    /// The secret is exposed only here, as Stripe's builder copies it into a
    /// sensitive `Authorization` header.
    pub fn new(secret: SecretValue) -> Result<Self> {
        let inner = ClientBuilder::new(secret.expose())
            .app_info(
                "abnegate-payments",
                Some(env!("CARGO_PKG_VERSION").to_owned()),
                Some("https://github.com/abnegate/crates".to_owned()),
            )
            .build()
            .map_err(Error::from_stripe)?;
        Ok(Self { inner })
    }
}

#[async_trait]
impl Payments for Client {
    async fn checkout(&self, request: CheckoutRequest) -> Result<Session> {
        if request.line_items.is_empty() {
            return Err(Error::invalid("checkout requires at least one line item"));
        }

        let line_items = request
            .line_items
            .iter()
            .map(|item| {
                let mut line = CreateCheckoutSessionLineItems::new();
                line.price = Some(item.price_id.clone());
                line.quantity = Some(item.quantity);
                line
            })
            .collect::<Vec<_>>();

        let mut create = CreateCheckoutSession::new()
            .success_url(request.success_url)
            .cancel_url(request.cancel_url)
            .line_items(line_items)
            .mode(request.mode.into_stripe());

        if let Some(customer_id) = request.customer_id {
            create = create.customer(customer_id);
        }
        if !request.metadata.is_empty() {
            create = create.metadata(request.metadata);
        }

        let session = create.send(&self.inner).await.map_err(Error::from_stripe)?;
        let url = session.url.ok_or_else(Error::incomplete)?;
        Ok(Session::new(session.id.as_str(), url))
    }

    async fn portal(&self, request: PortalRequest) -> Result<Session> {
        if request.customer_id.is_empty() {
            return Err(Error::invalid("the customer portal requires a customer id"));
        }

        let session = CreateBillingPortalSession::new()
            .customer(request.customer_id)
            .return_url(request.return_url)
            .send(&self.inner)
            .await
            .map_err(Error::from_stripe)?;
        Ok(Session::new(session.id.as_str(), session.url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_usable_secret_builds_a_client() {
        Client::new(SecretValue::new("sk_test_placeholder")).expect("visible ASCII builds");
    }

    #[test]
    fn a_secret_that_is_not_a_header_value_is_invalid_and_does_not_panic() {
        let error = Client::new(SecretValue::new("sk_test\nnewline")).expect_err("newline");
        assert!(matches!(error, Error::Invalid { .. }), "{error:?}");
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains("sk_test"), "{rendered}");
        assert!(!rendered.contains("newline"), "{rendered}");
    }
}
