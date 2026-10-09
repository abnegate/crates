# abnegate-payments

Stripe Checkout, the Customer Portal, and webhook signature verification,
without any application's billing types. A host maps its own identifiers through
Checkout `metadata` and reads them back from [`Event`].

`Client` is built from a [`SecretValue`](abnegate_secret::SecretValue). The
secret is exposed only as the Stripe client is constructed. Errors, `Debug` and
log lines never reproduce it.

## Features

- `testing`: `Fake`, a `Payments` that records instead of calling Stripe, and
  `Event::signed_header` for fixture bodies.

## Usage

```sh
cargo add abnegate-payments
```

```rust,no_run
use std::collections::HashMap;

use abnegate_payments::{CheckoutRequest, Client, Event, LineItem, Mode, Payments};
use abnegate_secret::SecretValue;

# async fn example() -> Result<(), abnegate_payments::Error> {
let payments = Client::new(SecretValue::new("sk_test_placeholder"))?;

let session = payments
    .checkout(
        CheckoutRequest::new(
            "https://example.test/billing/success",
            "https://example.test/billing/cancel",
            [LineItem::new("price_pro_monthly", 1)],
            Mode::Subscription,
        )
        .with_metadata(HashMap::from([("organization_id".into(), "org_1".into())])),
    )
    .await?;

let _ = session.url;
# let _ = session;
# Ok(())
# }
```

A webhook handler verifies the body it was posted, then matches the event:

```rust
use abnegate_payments::Event;
use abnegate_secret::SecretValue;

fn handle(payload: &str, signature: &str, secret: SecretValue) -> Result<(), abnegate_payments::Error> {
    match Event::from_payload(payload, signature, &secret)? {
        Event::CheckoutCompleted { metadata, .. } => {
            let _ = metadata.get("organization_id");
            Ok(())
        }
        Event::Unrecognised { .. } => Ok(()),
        _ => Ok(()),
    }
}
```

Events this crate does not sell a mapping for become [`Event::Unrecognised`].
Stripe's other two hundred event objects never appear in the public API.

## Credentials

The secret key and the webhook signing secret are
[`SecretValue`](abnegate_secret::SecretValue). Call [`SecretValue::expose`] only
at the Stripe client builder and at webhook verification. A Stripe error is
mapped into [`Error`](abnegate_payments::Error) so its `Display` cannot carry
the key, the webhook secret, or a request URL.
