use crate::address::literal;
use crate::address::must_not_be_fetched;
use crate::error::HttpError;
use crate::error::Result;
use reqwest::Url;
use reqwest::dns::Addrs;
use reqwest::dns::Name;
use reqwest::dns::Resolve;
use reqwest::dns::Resolving;
use std::net::SocketAddr;
use std::time::Duration;

/// Redirect hops a caller-supplied fetch may follow. Each one is validated, so
/// this bounds the chain rather than the trust.
const MAX_REDIRECTS: usize = 3;

/// Parse `raw` and reject schemes, hosts, and addresses that must not be
/// fetched on behalf of a caller.
pub fn validate_public_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|_| HttpError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(HttpError::UnsupportedScheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(HttpError::EmbeddedCredentials);
    }
    let host = url.host_str().ok_or(HttpError::MissingHost)?;

    if let Some(ip) = literal(host) {
        if must_not_be_fetched(ip) {
            return Err(HttpError::PrivateAddress);
        }
        return Ok(url);
    }

    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host == "metadata.google.internal"
    {
        return Err(HttpError::InternalHost);
    }
    Ok(url)
}

/// Resolves a name and refuses the addresses [`validate_public_url`] cannot see.
///
/// The URL check reads text; the address a name resolves to is chosen by DNS
/// afterwards, so a public hostname pointing at 127.0.0.1 or a LAN address
/// passes every textual check and is still an internal request. Refusing at
/// the connector is what makes the guard hold, because every request and every
/// redirect hop must resolve before it can connect.
struct PublicAddresses;

impl Resolve for PublicAddresses {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            let resolved = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let public = public_only(resolved, &host)?;
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

/// Keep only the addresses a caller-supplied fetch may connect to.
///
/// A name that answers with both a public and a private address is not
/// refused outright -- the private one is dropped, so a connector that falls
/// back through the list cannot arrive at it.
fn public_only(addresses: impl Iterator<Item = SocketAddr>, host: &str) -> Result<Vec<SocketAddr>> {
    let public: Vec<SocketAddr> = addresses
        .filter(|address| !must_not_be_fetched(address.ip()))
        .collect();

    if public.is_empty() {
        return Err(HttpError::UnfetchableResolution {
            host: host.to_string(),
        });
    }

    Ok(public)
}

/// A client builder for fetching a caller-supplied URL.
///
/// [`validate_public_url`] only answers for the URL it was handed. reqwest
/// issues the redirect hops itself, so the policy re-checks every hop: reading
/// the final address back afterwards refuses the disclosure but has already
/// made the request.
pub fn public_client_builder(timeout: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(timeout)
        .dns_resolver(PublicAddresses)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > MAX_REDIRECTS {
                attempt.error(HttpError::TooManyRedirects)
            } else {
                match validate_public_url(attempt.url().as_str()) {
                    Ok(_) => attempt.follow(),
                    Err(error) => attempt.error(error),
                }
            }
        }))
}

/// Build a client for fetching a caller-supplied URL.
pub fn public_client(timeout: Duration) -> Result<reqwest::Client> {
    Ok(public_client_builder(timeout).build()?)
}

/// Read at most `limit` bytes of `response`, refusing a body that does not fit
/// rather than buffering it whole and measuring it afterwards.
pub async fn read_capped(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(HttpError::OversizedBody { limit });
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| HttpError::UnreadableBody(error.without_url()))?
    {
        if body.len() + chunk.len() > limit {
            return Err(HttpError::OversizedBody { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(raw: &str) -> SocketAddr {
        raw.parse().expect("a socket address")
    }

    #[test]
    fn the_non_global_ranges_are_refused_as_urls() {
        for raw in [
            "100.64.0.1",
            "198.18.0.1",
            "192.0.2.1",
            "240.0.0.1",
            "[64:ff9b::7f00:1]",
            "[2002:7f00:1::]",
            "[fec0::1]",
        ] {
            assert!(
                matches!(
                    validate_public_url(&format!("http://{raw}/")),
                    Err(HttpError::PrivateAddress)
                ),
                "{raw} passed the URL check"
            );
        }
    }

    /// `validate_public_url` reads text. Which address a name answers with is
    /// chosen afterwards by DNS, so a public hostname pointing at loopback
    /// passes every textual check and is still an internal request.
    #[test]
    fn a_name_answering_only_with_private_addresses_is_refused() {
        let error = public_only(
            [address("127.0.0.1:0"), address("[::1]:0")].into_iter(),
            "inside.example",
        )
        .expect_err("loopback must not be fetched");
        assert!(error.to_string().contains("inside.example"), "{error}");
    }

    #[test]
    fn a_name_answering_with_a_public_address_is_allowed() {
        let public = public_only([address("93.184.216.34:0")].into_iter(), "example.test")
            .expect("a public address is fetchable");
        assert_eq!(public, vec![address("93.184.216.34:0")]);
    }

    /// The private address is dropped rather than the whole answer refused, so
    /// a connector working down the list cannot fall back onto it.
    #[test]
    fn a_private_address_beside_a_public_one_is_dropped() {
        let public = public_only(
            [address("127.0.0.1:0"), address("93.184.216.34:0")].into_iter(),
            "both.example",
        )
        .expect("the public address stands");
        assert_eq!(
            public,
            vec![address("93.184.216.34:0")],
            "the loopback address survived alongside the public one"
        );
    }

    #[tokio::test]
    async fn the_client_resolver_refuses_a_name_that_answers_with_loopback() {
        let name: Name = "localhost".parse().expect("a resolvable name");
        let error = Resolve::resolve(&PublicAddresses, name)
            .await
            .err()
            .expect("localhost must not be fetchable");
        assert!(error.to_string().contains("must not be fetched"), "{error}");
    }

    #[test]
    fn rejects_private_and_internal_targets() {
        for url in [
            "http://127.0.0.1/",
            "http://10.0.0.1/",
            "http://localhost/admin",
            "http://169.254.169.254/latest",
            "file:///etc/passwd",
            "https://user:pass@example.com/",
        ] {
            assert!(validate_public_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn rejects_ipv6_and_trailing_dot_spellings_of_the_same_targets() {
        for url in [
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:169.254.169.254]/latest/meta-data/",
            "http://[fd00::1]/",
            "http://[fe80::1]/",
            "http://localhost./",
            "http://LOCALHOST/",
            "http://127.0.0.1./",
        ] {
            assert!(validate_public_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn accepts_public_https() {
        assert_eq!(
            validate_public_url("https://example.com/docs")
                .expect("a public URL")
                .as_str(),
            "https://example.com/docs"
        );
    }

    #[test]
    fn a_guarded_client_builds() {
        assert!(public_client(Duration::from_secs(5)).is_ok());
    }

    async fn body_of(length: usize) -> reqwest::Response {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("b".repeat(length)))
            .mount(&server)
            .await;

        reqwest::get(server.uri())
            .await
            .expect("the mock server answers")
    }

    #[tokio::test]
    async fn a_body_inside_the_cap_is_read_whole() {
        let body = read_capped(body_of(64).await, 128)
            .await
            .expect("64 bytes fit under 128");

        assert_eq!(body.len(), 64);
    }

    #[tokio::test]
    async fn a_body_the_size_of_the_cap_is_still_read() {
        let body = read_capped(body_of(128).await, 128)
            .await
            .expect("the cap is inclusive");

        assert_eq!(body.len(), 128);
    }

    /// A declared length is refused before the body is buffered, so an
    /// oversized response never costs the memory it claimed.
    #[tokio::test]
    async fn a_body_over_the_cap_is_refused() {
        let error = read_capped(body_of(512).await, 128)
            .await
            .expect_err("512 bytes do not fit under 128");

        assert!(
            matches!(error, HttpError::OversizedBody { limit: 128 }),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_guarded_client_refuses_a_loopback_target() {
        let client = public_client(Duration::from_secs(5)).expect("a guarded client");

        assert!(client.get("http://localhost:1/").send().await.is_err());
    }
}
