//! One price on a Checkout Session.

/// A Stripe Price and how many of it to sell.
///
/// A field may be added in a minor release, so a caller builds this with
/// [`LineItem::new`] rather than a literal.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct LineItem {
    /// The Stripe Price id, such as `price_...`.
    pub price_id: String,
    /// How many of the price to sell. Stripe omits this on metered prices;
    /// a host that sells those still sends 1 here and Stripe ignores it when
    /// the price is metered.
    pub quantity: u64,
}

impl LineItem {
    /// Sell `quantity` of the Stripe Price `price_id`.
    pub fn new(price_id: impl Into<String>, quantity: u64) -> Self {
        Self {
            price_id: price_id.into(),
            quantity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_item_keeps_the_price_and_quantity() {
        let item = LineItem::new("price_pro_monthly", 2);
        assert_eq!(item.price_id, "price_pro_monthly");
        assert_eq!(item.quantity, 2);
    }
}
