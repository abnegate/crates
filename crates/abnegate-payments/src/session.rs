//! A hosted Stripe session the customer is sent to.

/// The id and URL of a Checkout or Customer Portal session.
///
/// A field may be added in a minor release, so a caller, and a
/// [`Payments`](crate::Payments) test double, builds this with
/// [`Session::new`] rather than a literal.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Session {
    /// Stripe's id for the session (`cs_...` or `bps_...`).
    pub id: String,
    /// The hosted URL the customer is redirected to.
    pub url: String,
}

impl Session {
    /// A session Stripe (or a test double) just created.
    pub fn new(id: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            url: url.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_keeps_the_id_and_url() {
        let session = Session::new("cs_test", "https://checkout.stripe.com/c/pay/cs_test");
        assert_eq!(session.id, "cs_test");
        assert_eq!(session.url, "https://checkout.stripe.com/c/pay/cs_test");
    }
}
