//! Payments that are recorded rather than sent.

use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use async_trait::async_trait;

use crate::checkout_request::CheckoutRequest;
use crate::error::Error;
use crate::error::Result;
use crate::payments::Payments;
use crate::portal_request::PortalRequest;
use crate::session::Session;

/// A [`Payments`] that records instead of calling Stripe.
///
/// It takes and answers exactly what [`Client`](crate::Client) does, so a
/// caller under test can hold either and assert on the request. Queue a
/// [`Session`] with [`Fake::enqueue_checkout`] or [`Fake::enqueue_portal`], or
/// a failure with [`Fake::fail_checkout`] / [`Fake::fail_portal`]. With
/// nothing queued, each call returns a fresh session whose id is unique to
/// this fake.
#[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
#[derive(Debug, Default)]
pub struct Fake {
    inner: Mutex<Inner>,
    next_checkout: AtomicU64,
    next_portal: AtomicU64,
}

#[derive(Debug, Default)]
struct Inner {
    checkouts: Vec<CheckoutRequest>,
    portals: Vec<PortalRequest>,
    checkout_sessions: Vec<Session>,
    portal_sessions: Vec<Session>,
    checkout_errors: Vec<Error>,
    portal_errors: Vec<Error>,
}

impl Fake {
    /// A fake that has recorded nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The next checkout returns `session` instead of a generated one.
    pub fn enqueue_checkout(&self, session: Session) {
        self.inner
            .lock()
            .expect("fake mutex")
            .checkout_sessions
            .push(session);
    }

    /// The next portal call returns `session` instead of a generated one.
    pub fn enqueue_portal(&self, session: Session) {
        self.inner
            .lock()
            .expect("fake mutex")
            .portal_sessions
            .push(session);
    }

    /// The next checkout returns `error`.
    pub fn fail_checkout(&self, error: Error) {
        self.inner
            .lock()
            .expect("fake mutex")
            .checkout_errors
            .push(error);
    }

    /// The next portal call returns `error`.
    pub fn fail_portal(&self, error: Error) {
        self.inner
            .lock()
            .expect("fake mutex")
            .portal_errors
            .push(error);
    }

    /// Every checkout request so far, oldest first.
    pub fn checkouts(&self) -> Vec<CheckoutRequest> {
        self.inner.lock().expect("fake mutex").checkouts.clone()
    }

    /// Every portal request so far, oldest first.
    pub fn portals(&self) -> Vec<PortalRequest> {
        self.inner.lock().expect("fake mutex").portals.clone()
    }
}

#[async_trait]
impl Payments for Fake {
    async fn checkout(&self, request: CheckoutRequest) -> Result<Session> {
        if request.line_items().is_empty() {
            return Err(Error::invalid("checkout requires at least one line item"));
        }
        let mut inner = self.inner.lock().expect("fake mutex");
        if !inner.checkout_errors.is_empty() {
            inner.checkouts.push(request);
            return Err(inner.checkout_errors.remove(0));
        }
        let session = if inner.checkout_sessions.is_empty() {
            let n = self.next_checkout.fetch_add(1, Ordering::Relaxed) + 1;
            Session::new(
                format!("cs_test_{n}"),
                format!("https://checkout.stripe.com/c/pay/cs_test_{n}"),
            )
        } else {
            inner.checkout_sessions.remove(0)
        };
        inner.checkouts.push(request);
        Ok(session)
    }

    async fn portal(&self, request: PortalRequest) -> Result<Session> {
        if request.customer_id().is_empty() {
            return Err(Error::invalid("the customer portal requires a customer id"));
        }
        let mut inner = self.inner.lock().expect("fake mutex");
        if !inner.portal_errors.is_empty() {
            inner.portals.push(request);
            return Err(inner.portal_errors.remove(0));
        }
        let session = if inner.portal_sessions.is_empty() {
            let n = self.next_portal.fetch_add(1, Ordering::Relaxed) + 1;
            Session::new(
                format!("bps_test_{n}"),
                format!("https://billing.stripe.com/p/session/bps_test_{n}"),
            )
        } else {
            inner.portal_sessions.remove(0)
        };
        inner.portals.push(request);
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_item::LineItem;
    use crate::mode::Mode;
    use std::sync::Arc;

    fn checkout() -> CheckoutRequest {
        CheckoutRequest::new(
            "https://example.test/success",
            "https://example.test/cancel",
            [LineItem::new("price_pro", 1)],
            Mode::Subscription,
        )
    }

    #[tokio::test]
    async fn the_fake_records_checkout_and_returns_a_session() {
        let fake = Fake::new();
        fake.enqueue_checkout(Session::new(
            "cs_queued",
            "https://checkout.stripe.com/c/pay/cs_queued",
        ));

        let session = fake.checkout(checkout()).await.expect("queued");
        assert_eq!(session.id, "cs_queued");
        assert_eq!(fake.checkouts().len(), 1);
        assert_eq!(fake.checkouts()[0].line_items()[0].price_id, "price_pro");
    }

    #[tokio::test]
    async fn the_fake_records_portal_and_can_fail() {
        let fake = Fake::new();
        fake.fail_portal(Error::refused(400));

        let error = fake
            .portal(PortalRequest::new(
                "cus_123",
                "https://example.test/billing",
            ))
            .await
            .expect_err("queued");
        assert!(matches!(error, Error::Refused { status: 400, .. }));
        assert_eq!(fake.portals().len(), 1);
    }

    #[tokio::test]
    async fn the_fake_stands_in_wherever_payments_is_expected() {
        let payments: Arc<dyn Payments> = Arc::new(Fake::new());
        let session = payments.checkout(checkout()).await.expect("generated");
        assert!(session.id.starts_with("cs_test_"));
        assert!(session.url.starts_with("https://checkout.stripe.com/"));
    }

    #[tokio::test]
    async fn empty_line_items_are_invalid_on_the_fake_too() {
        let fake = Fake::new();
        let error = fake
            .checkout(CheckoutRequest::new(
                "https://example.test/success",
                "https://example.test/cancel",
                [],
                Mode::Payment,
            ))
            .await
            .expect_err("empty");
        assert!(matches!(error, Error::Invalid { .. }));
        assert!(fake.checkouts().is_empty());
    }
}
