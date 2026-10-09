//! Verified Stripe webhook events, mapped onto the handful a host bills from.

use std::collections::HashMap;

use abnegate_secret::SecretValue;
use stripe_types::AsCursor;
use stripe_types::Expandable;
use stripe_webhook::EventObject;
use stripe_webhook::Webhook;

use crate::error::Error;
use crate::error::Result;

/// A verified Stripe event this crate knows how to bill from.
///
/// [`Event::from_payload`] checks the signature, then maps Checkout,
/// subscription and invoice events. Every other type Stripe sends is
/// [`Event::Unrecognised`], so Stripe's remaining event objects never appear
/// in this crate's public API.
///
/// A variant may gain a field in a minor release. A pattern outside this crate
/// ends in `..`. A host that needs to build an event in a test uses the
/// constructors.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// `checkout.session.completed`.
    #[non_exhaustive]
    CheckoutCompleted {
        /// Stripe's event id (`evt_...`).
        event_id: String,
        /// The Checkout Session id (`cs_...`).
        session_id: String,
        /// The Customer Stripe created or reused, when the session has one.
        customer_id: Option<String>,
        /// The Subscription the session created, when it ran in subscription
        /// mode.
        subscription_id: Option<String>,
        /// Opaque keys the host set on the Checkout Session.
        metadata: HashMap<String, String>,
    },
    /// `customer.subscription.updated`.
    #[non_exhaustive]
    SubscriptionUpdated {
        /// Stripe's event id (`evt_...`).
        event_id: String,
        /// The Subscription id (`sub_...`).
        subscription_id: String,
        /// The Customer who owns the subscription.
        customer_id: String,
        /// Stripe's status string (`active`, `past_due`, `canceled`, ...).
        status: String,
        /// Opaque keys stored on the subscription.
        metadata: HashMap<String, String>,
        /// Start of the current period, Unix seconds, taken from the
        /// subscription items when Stripe reports them.
        current_period_start: Option<i64>,
        /// End of the current period, Unix seconds, taken from the
        /// subscription items when Stripe reports them.
        current_period_end: Option<i64>,
    },
    /// `customer.subscription.deleted`.
    #[non_exhaustive]
    SubscriptionDeleted {
        /// Stripe's event id (`evt_...`).
        event_id: String,
        /// The Subscription id (`sub_...`).
        subscription_id: String,
        /// The Customer who owned the subscription.
        customer_id: String,
        /// Opaque keys stored on the subscription.
        metadata: HashMap<String, String>,
    },
    /// `invoice.paid`.
    #[non_exhaustive]
    InvoicePaid {
        /// Stripe's event id (`evt_...`).
        event_id: String,
        /// The Invoice id (`in_...`), when Stripe included one.
        invoice_id: Option<String>,
        /// The Customer the invoice was for, when Stripe included one.
        customer_id: Option<String>,
        /// The Subscription the invoice billed, when it billed one.
        subscription_id: Option<String>,
        /// Amount paid, in the currency's smallest unit.
        amount_paid: i64,
    },
    /// `invoice.payment_failed`.
    #[non_exhaustive]
    InvoicePaymentFailed {
        /// Stripe's event id (`evt_...`).
        event_id: String,
        /// The Invoice id (`in_...`), when Stripe included one.
        invoice_id: Option<String>,
        /// The Customer the invoice was for, when Stripe included one.
        customer_id: Option<String>,
        /// The Subscription the invoice billed, when it billed one.
        subscription_id: Option<String>,
        /// Amount paid, in the currency's smallest unit. Zero when nothing
        /// collected.
        amount_paid: i64,
    },
    /// A verified event this crate does not map.
    #[non_exhaustive]
    Unrecognised {
        /// Stripe's event id (`evt_...`).
        event_id: String,
        /// Stripe's event type string, such as `charge.succeeded`.
        kind: String,
    },
}

impl Event {
    /// Verify `payload` against `signature` with `secret`, then map it.
    ///
    /// `secret` is exposed only for the HMAC Stripe documents. A mismatch is
    /// [`Error::Unverified`]; a timestamp outside the replay window is
    /// [`Error::Stale`].
    pub fn from_payload(payload: &str, signature: &str, secret: &SecretValue) -> Result<Self> {
        let event = Webhook::construct_event(payload, signature, secret.expose())
            .map_err(Error::from_webhook)?;
        Ok(Self::from_stripe(event))
    }

    /// A Checkout Session that completed.
    pub fn checkout_completed(
        event_id: impl Into<String>,
        session_id: impl Into<String>,
        customer_id: Option<String>,
        subscription_id: Option<String>,
        metadata: HashMap<String, String>,
    ) -> Self {
        Self::CheckoutCompleted {
            event_id: event_id.into(),
            session_id: session_id.into(),
            customer_id,
            subscription_id,
            metadata,
        }
    }

    /// A subscription that changed plan, status or period.
    pub fn subscription_updated(
        event_id: impl Into<String>,
        subscription_id: impl Into<String>,
        customer_id: impl Into<String>,
        status: impl Into<String>,
        metadata: HashMap<String, String>,
        current_period_start: Option<i64>,
        current_period_end: Option<i64>,
    ) -> Self {
        Self::SubscriptionUpdated {
            event_id: event_id.into(),
            subscription_id: subscription_id.into(),
            customer_id: customer_id.into(),
            status: status.into(),
            metadata,
            current_period_start,
            current_period_end,
        }
    }

    /// A subscription that was deleted.
    pub fn subscription_deleted(
        event_id: impl Into<String>,
        subscription_id: impl Into<String>,
        customer_id: impl Into<String>,
        metadata: HashMap<String, String>,
    ) -> Self {
        Self::SubscriptionDeleted {
            event_id: event_id.into(),
            subscription_id: subscription_id.into(),
            customer_id: customer_id.into(),
            metadata,
        }
    }

    /// An invoice that was paid.
    pub fn invoice_paid(
        event_id: impl Into<String>,
        invoice_id: Option<String>,
        customer_id: Option<String>,
        subscription_id: Option<String>,
        amount_paid: i64,
    ) -> Self {
        Self::InvoicePaid {
            event_id: event_id.into(),
            invoice_id,
            customer_id,
            subscription_id,
            amount_paid,
        }
    }

    /// An invoice whose payment failed.
    pub fn invoice_payment_failed(
        event_id: impl Into<String>,
        invoice_id: Option<String>,
        customer_id: Option<String>,
        subscription_id: Option<String>,
        amount_paid: i64,
    ) -> Self {
        Self::InvoicePaymentFailed {
            event_id: event_id.into(),
            invoice_id,
            customer_id,
            subscription_id,
            amount_paid,
        }
    }

    /// A verified event this crate does not map.
    pub fn unrecognised(event_id: impl Into<String>, kind: impl Into<String>) -> Self {
        Self::Unrecognised {
            event_id: event_id.into(),
            kind: kind.into(),
        }
    }

    /// A `Stripe-Signature` header for `payload`, signed with `secret`.
    ///
    /// For tests that feed [`Event::from_payload`]. The header is valid at
    /// the moment it is produced.
    #[cfg(feature = "testing")]
    #[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
    pub fn signed_header(payload: &str, secret: &SecretValue) -> String {
        Webhook::generate_test_header(payload, secret.expose(), None)
    }

    fn from_stripe(event: stripe_webhook::Event) -> Self {
        let event_id = event.id.as_str().to_owned();
        match event.data.object {
            EventObject::CheckoutSessionCompleted(session) => Self::checkout_completed(
                event_id,
                session.id.as_str(),
                session.customer.as_ref().map(expandable_id),
                session.subscription.as_ref().map(expandable_id),
                session.metadata.unwrap_or_default(),
            ),
            EventObject::CustomerSubscriptionUpdated(subscription) => {
                let (current_period_start, current_period_end) = subscription_period(&subscription);
                Self::subscription_updated(
                    event_id,
                    subscription.id.as_str(),
                    expandable_id(&subscription.customer),
                    subscription.status.as_str(),
                    subscription.metadata,
                    current_period_start,
                    current_period_end,
                )
            }
            EventObject::CustomerSubscriptionDeleted(subscription) => Self::subscription_deleted(
                event_id,
                subscription.id.as_str(),
                expandable_id(&subscription.customer),
                subscription.metadata,
            ),
            EventObject::InvoicePaid(invoice) => Self::invoice_paid(
                event_id,
                invoice.id.as_ref().map(|id| id.as_str().to_owned()),
                invoice.customer.as_ref().map(expandable_id),
                invoice.subscription.as_ref().map(expandable_id),
                invoice.amount_paid,
            ),
            EventObject::InvoicePaymentFailed(invoice) => Self::invoice_payment_failed(
                event_id,
                invoice.id.as_ref().map(|id| id.as_str().to_owned()),
                invoice.customer.as_ref().map(expandable_id),
                invoice.subscription.as_ref().map(expandable_id),
                invoice.amount_paid,
            ),
            _ => Self::unrecognised(event_id, event.type_.as_str()),
        }
    }
}

fn expandable_id<T: stripe_types::Object>(expandable: &Expandable<T>) -> String
where
    T::Id: stripe_types::AsCursor,
{
    expandable.id().as_cursor().to_owned()
}

fn subscription_period(subscription: &stripe_shared::Subscription) -> (Option<i64>, Option<i64>) {
    let start = subscription
        .items
        .data
        .iter()
        .map(|item| item.current_period_start)
        .min();
    let end = subscription
        .items
        .data
        .iter()
        .map(|item| item.current_period_end)
        .max();
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "whsec_test_secret";

    fn secret() -> SecretValue {
        SecretValue::new(SECRET)
    }

    fn header(payload: &str) -> String {
        Webhook::generate_test_header(payload, SECRET, None)
    }

    fn envelope(event_id: &str, kind: &str, object: &str) -> String {
        format!(
            r#"{{
                "id": "{event_id}",
                "object": "event",
                "api_version": "2017-05-25",
                "created": 1492774577,
                "livemode": false,
                "pending_webhooks": 1,
                "data": {{ "object": {object} }},
                "type": "{kind}"
            }}"#
        )
    }

    #[test]
    fn a_signed_external_account_event_is_unrecognised() {
        let payload = envelope(
            "evt_test",
            "account.external_account.created",
            r#"{
                "object": "bank_account",
                "country": "us",
                "currency": "usd",
                "id": "ba_test",
                "last4": "6789",
                "status": "verified"
            }"#,
        );
        let event = Event::from_payload(&payload, &header(&payload), &secret()).expect("verified");
        assert_eq!(
            event,
            Event::unrecognised("evt_test", "account.external_account.created")
        );
    }

    #[test]
    fn checkout_completed_round_trips_metadata() {
        let payload = envelope(
            "evt_checkout",
            "checkout.session.completed",
            r#"{
                "id": "cs_test_123",
                "object": "checkout.session",
                "automatic_tax": { "enabled": false },
                "created": 1700000000,
                "custom_fields": [],
                "custom_text": {},
                "customer": "cus_test",
                "expires_at": 1700086400,
                "livemode": false,
                "metadata": { "organization_id": "org_1" },
                "mode": "subscription",
                "payment_method_types": ["card"],
                "payment_status": "paid",
                "shipping_options": [],
                "subscription": "sub_test",
                "url": "https://checkout.stripe.com/c/pay/cs_test_123"
            }"#,
        );
        let event = Event::from_payload(&payload, &header(&payload), &secret()).expect("verified");
        match event {
            Event::CheckoutCompleted {
                event_id,
                session_id,
                customer_id,
                subscription_id,
                metadata,
                ..
            } => {
                assert_eq!(event_id, "evt_checkout");
                assert_eq!(session_id, "cs_test_123");
                assert_eq!(customer_id.as_deref(), Some("cus_test"));
                assert_eq!(subscription_id.as_deref(), Some("sub_test"));
                assert_eq!(
                    metadata.get("organization_id").map(String::as_str),
                    Some("org_1")
                );
            }
            other => panic!("expected checkout completed, got {other:?}"),
        }
    }

    #[test]
    fn a_bad_signature_is_unverified_and_does_not_echo_the_secret() {
        let payload = envelope(
            "evt_test",
            "account.external_account.created",
            r#"{
                "object": "bank_account",
                "country": "us",
                "currency": "usd",
                "id": "ba_test",
                "last4": "6789",
                "status": "verified"
            }"#,
        );
        let error = Event::from_payload(&payload, "t=1,v1=deadbeef", &secret())
            .expect_err("the signature is wrong");
        assert!(matches!(error, Error::Unverified));
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains(SECRET), "{rendered}");
    }

    #[test]
    fn a_stale_signature_is_stale() {
        let payload = envelope(
            "evt_test",
            "account.external_account.created",
            r#"{
                "object": "bank_account",
                "country": "us",
                "currency": "usd",
                "id": "ba_test",
                "last4": "6789",
                "status": "verified"
            }"#,
        );
        let header = Webhook::generate_test_header(&payload, SECRET, Some(1_492_774_577));
        let error =
            Event::from_payload(&payload, &header, &secret()).expect_err("the timestamp is old");
        assert!(matches!(error, Error::Stale));
    }

    #[test]
    fn a_body_that_is_not_an_event_is_malformed() {
        let payload = "{}";
        let error =
            Event::from_payload(payload, &header(payload), &secret()).expect_err("not an event");
        assert!(matches!(error, Error::Malformed));
    }

    #[test]
    fn constructors_fill_their_variants() {
        let metadata = HashMap::from([("organization_id".into(), "org_1".into())]);
        assert!(matches!(
            Event::checkout_completed("evt", "cs", None, None, metadata.clone()),
            Event::CheckoutCompleted { .. }
        ));
        assert!(matches!(
            Event::subscription_updated(
                "evt",
                "sub",
                "cus",
                "active",
                metadata.clone(),
                Some(1),
                Some(2)
            ),
            Event::SubscriptionUpdated {
                current_period_start: Some(1),
                current_period_end: Some(2),
                ..
            }
        ));
        assert!(matches!(
            Event::invoice_paid("evt", Some("in_1".into()), None, None, 2900),
            Event::InvoicePaid {
                amount_paid: 2900,
                ..
            }
        ));
    }
}
