//! A request to open a Stripe Customer Portal session.

/// Everything Stripe needs to open the Customer Portal for one customer.
///
/// Stripe requires a customer id. Fields are private so a host cannot omit
/// it with a struct literal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortalRequest {
    pub(crate) customer_id: String,
    pub(crate) return_url: String,
}

impl PortalRequest {
    /// Open the portal for `customer_id`, then send them to `return_url`.
    pub fn new(customer_id: impl Into<String>, return_url: impl Into<String>) -> Self {
        Self {
            customer_id: customer_id.into(),
            return_url: return_url.into(),
        }
    }

    /// The Stripe Customer the portal is for.
    pub fn customer_id(&self) -> &str {
        &self.customer_id
    }

    /// The URL the portal's return link sends the customer to.
    pub fn return_url(&self) -> &str {
        &self.return_url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_portal_request_keeps_the_customer_and_return_url() {
        let request = PortalRequest::new("cus_123", "https://example.test/billing");
        assert_eq!(request.customer_id(), "cus_123");
        assert_eq!(request.return_url(), "https://example.test/billing");
    }
}
