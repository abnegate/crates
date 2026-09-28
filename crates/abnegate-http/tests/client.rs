//! An [`HttpClient`] standing in for the network, written the way a caller's
//! own test suite writes one.

use abnegate_http::{Error, HttpClient, HttpResponse, Result};
use async_trait::async_trait;

/// Refuses every request the way the guarded transport refuses one.
enum Refusing {
    Resolution,
    Body,
}

#[async_trait]
impl HttpClient for Refusing {
    async fn get(&self, _url: &str, _headers: Vec<(&str, String)>) -> Result<HttpResponse> {
        match self {
            Self::Resolution => Err(Error::unfetchable_resolution("inside.example")),
            Self::Body => Err(Error::oversized_body(1_024)),
        }
    }
}

#[tokio::test]
async fn a_double_refuses_a_name_that_resolves_inside_the_deployment() {
    let error = Refusing::Resolution
        .get("https://inside.example/", Vec::new())
        .await
        .expect_err("the double refuses");

    assert!(
        matches!(&error, Error::UnfetchableResolution { host, .. } if host == "inside.example"),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "inside.example resolves only to addresses that must not be fetched."
    );
}

#[tokio::test]
async fn a_double_refuses_a_body_over_the_limit() {
    let error = Refusing::Body
        .get("https://example.test/", Vec::new())
        .await
        .expect_err("the double refuses");

    assert!(
        matches!(error, Error::OversizedBody { limit: 1_024, .. }),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "The response body is larger than 1024 bytes."
    );
}
