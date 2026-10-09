#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! Stripe Checkout, Customer Portal, and signed webhook events.
//!
//! [`Client`] talks to Stripe. [`Payments`] is the trait a host depends on, so
//! a test can hold `Fake` instead. [`Event::from_payload`]
//! verifies a webhook body and maps the events a billing host needs; every
//! other Stripe event is [`Event::Unrecognised`].
//!
//! ```no_run
//! # async fn example() -> Result<(), abnegate_payments::Error> {
//! use abnegate_payments::{CheckoutRequest, Client, LineItem, Mode, Payments};
//! use abnegate_secret::SecretValue;
//!
//! let payments = Client::new(SecretValue::new("sk_test_placeholder"))?;
//! let session = payments
//!     .checkout(CheckoutRequest::new(
//!         "https://example.test/success",
//!         "https://example.test/cancel",
//!         [LineItem::new("price_pro_monthly", 1)],
//!         Mode::Subscription,
//!     ))
//!     .await?;
//! let _ = session.url;
//! # Ok(())
//! # }
//! ```
//!
//! # Features
//!
//! - `testing`: `Fake`, a [`Payments`] that records instead of sending, and
//!   `Event::signed_header` for fixture bodies.
//!
//! # Credentials
//!
//! The secret key and the webhook signing secret are
//! [`SecretValue`](abnegate_secret::SecretValue). They are exposed only as the
//! Stripe client is built and as a webhook is verified. No error, `Debug` or
//! log line in this crate reproduces them.

mod checkout_request;
mod client;
mod error;
mod event;
#[cfg(feature = "testing")]
mod fake;
mod line_item;
mod mode;
mod payments;
mod portal_request;
mod session;

pub use crate::checkout_request::CheckoutRequest;
pub use crate::client::Client;
pub use crate::error::Error;
pub use crate::error::Result;
pub use crate::event::Event;
#[cfg(feature = "testing")]
pub use crate::fake::Fake;
pub use crate::line_item::LineItem;
pub use crate::mode::Mode;
pub use crate::payments::Payments;
pub use crate::portal_request::PortalRequest;
pub use crate::session::Session;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
