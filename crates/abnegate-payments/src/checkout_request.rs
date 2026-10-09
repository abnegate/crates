//! A request to open a Stripe Checkout Session.

use std::collections::HashMap;

use crate::line_item::LineItem;
use crate::mode::Mode;

/// Everything Stripe needs to open a Checkout Session.
///
/// Fields are private so a host cannot construct a request Stripe would
/// refuse for a missing URL. Optional customer and metadata are set with
/// [`CheckoutRequest::with_customer_id`] and
/// [`CheckoutRequest::with_metadata`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckoutRequest {
    pub(crate) success_url: String,
    pub(crate) cancel_url: String,
    pub(crate) line_items: Vec<LineItem>,
    pub(crate) mode: Mode,
    pub(crate) customer_id: Option<String>,
    pub(crate) metadata: HashMap<String, String>,
}

impl CheckoutRequest {
    /// Open Checkout for `line_items` in `mode`, then send the customer to
    /// `success_url` or `cancel_url`.
    pub fn new(
        success_url: impl Into<String>,
        cancel_url: impl Into<String>,
        line_items: impl IntoIterator<Item = LineItem>,
        mode: Mode,
    ) -> Self {
        Self {
            success_url: success_url.into(),
            cancel_url: cancel_url.into(),
            line_items: line_items.into_iter().collect(),
            mode,
            customer_id: None,
            metadata: HashMap::new(),
        }
    }

    /// Prefill Checkout with an existing Stripe Customer.
    pub fn with_customer_id(mut self, customer_id: impl Into<String>) -> Self {
        self.customer_id = Some(customer_id.into());
        self
    }

    /// Opaque keys Stripe stores on the session and echoes on
    /// [`Event::CheckoutCompleted`](crate::Event::CheckoutCompleted).
    pub fn with_metadata(mut self, metadata: HashMap<String, String>) -> Self {
        self.metadata = metadata;
        self
    }

    /// The URL Stripe sends the customer to after a successful payment.
    pub fn success_url(&self) -> &str {
        &self.success_url
    }

    /// The URL Stripe sends the customer to if they abandon Checkout.
    pub fn cancel_url(&self) -> &str {
        &self.cancel_url
    }

    /// The prices on the session.
    pub fn line_items(&self) -> &[LineItem] {
        &self.line_items
    }

    /// Whether the session is a one-time payment or a subscription.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The existing Stripe Customer, when the host already has one.
    pub fn customer_id(&self) -> Option<&str> {
        self.customer_id.as_deref()
    }

    /// Opaque keys the host will read back from the completed session.
    pub fn metadata(&self) -> &HashMap<String, String> {
        &self.metadata
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_fields_start_empty_and_the_with_methods_fill_them() {
        let request = CheckoutRequest::new(
            "https://example.test/success",
            "https://example.test/cancel",
            [LineItem::new("price_pro", 1)],
            Mode::Subscription,
        );
        assert!(request.customer_id().is_none());
        assert!(request.metadata().is_empty());

        let request = request
            .with_customer_id("cus_123")
            .with_metadata(HashMap::from([("organization_id".into(), "org_1".into())]));
        assert_eq!(request.customer_id(), Some("cus_123"));
        assert_eq!(
            request
                .metadata()
                .get("organization_id")
                .map(String::as_str),
            Some("org_1")
        );
        assert_eq!(request.line_items()[0].price_id, "price_pro");
        assert_eq!(request.mode(), Mode::Subscription);
    }
}
