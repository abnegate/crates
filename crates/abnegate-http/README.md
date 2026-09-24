# abnegate-http

The pieces a service needs to talk to the network safely. `HttpClient` is the
trait a source adapter or an API client depends on, so a test can stand in for
the transport, and `ReqwestHttpClient` is the transport itself. `PublicClient`,
built by `public_client`, fetches a caller-supplied URL without reaching back
inside the deployment: it checks the URL with `validate_public_url` before every
request, every address a name resolves to, and every redirect hop.
`RateLimiter` counts requests against whatever key a caller limits by, `Backoff`
spaces retries out, and `is_rate_limit_error` and `is_hard_error` read a failure
message to decide which of the two a failure deserves.

## Features

None.

## Usage

```sh
cargo add abnegate-http
```

```rust,no_run
use std::time::Duration;

use abnegate_http::{Backoff, Error, is_rate_limit_error, public_client, validate_public_url};

async fn fetch() -> Result<(), Error> {
    assert!(validate_public_url("http://169.254.169.254/latest").is_err());

    let client = public_client(Duration::from_secs(10))?;
    match client.get("https://example.com/docs")?.send().await {
        Ok(response) => println!("{}", response.status()),
        Err(error) if is_rate_limit_error(&error.to_string()) => {
            println!("retry in {:?}", Backoff::default().delay(0));
        }
        Err(error) => return Err(error),
    }
    Ok(())
}
```
