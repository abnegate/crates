//! Checkout session mode.

/// How Stripe should treat the line items on a Checkout Session.
///
/// A variant may be added in a minor release. A pattern outside this crate
/// ends in `_`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Mode {
    /// A one-time payment, such as a credit pack.
    Payment,
    /// A recurring subscription.
    Subscription,
}

impl Mode {
    pub(crate) fn into_stripe(self) -> stripe_shared::CheckoutSessionMode {
        match self {
            Self::Payment => stripe_shared::CheckoutSessionMode::Payment,
            Self::Subscription => stripe_shared::CheckoutSessionMode::Subscription,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_mode_maps_onto_stripe() {
        assert_eq!(
            Mode::Payment.into_stripe().as_str(),
            stripe_shared::CheckoutSessionMode::Payment.as_str()
        );
        assert_eq!(
            Mode::Subscription.into_stripe().as_str(),
            stripe_shared::CheckoutSessionMode::Subscription.as_str()
        );
    }
}
