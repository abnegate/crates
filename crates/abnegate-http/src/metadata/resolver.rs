use crate::address::literal;
use crate::error::Error;
use crate::error::Result;
use crate::metadata::is_address;
use reqwest::dns::Addrs;
use reqwest::dns::Name;
use reqwest::dns::Resolve;
use reqwest::dns::Resolving;
use std::net::SocketAddr;

/// Resolves a name and drops the link-local and cloud metadata addresses it
/// answers with, so a name that passed [`is_host`](crate::metadata::is_host)
/// when it was saved cannot point at them by the time a request is sent.
///
/// A name that answers with nothing else is refused with
/// [`Error::MetadataResolution`]; loopback, LAN and every other address stay
/// reachable. Hand it to [`reqwest::ClientBuilder::dns_resolver`] to guard a
/// client of your own, or let
/// [`TargetPolicy::ExceptMetadata`](crate::TargetPolicy) install it on a
/// [`PublicClient`](crate::PublicClient). reqwest consults a resolver only for
/// names, never for IP literals, and a proxy resolves the target itself, so a
/// client of your own also checks each URL with
/// [`is_url`](crate::metadata::is_url) and switches proxies off.
///
/// ```
/// use abnegate_http::metadata::MetadataResolver;
/// use reqwest::Client;
/// use reqwest::redirect::Policy;
///
/// let client = Client::builder()
///     .no_proxy()
///     .redirect(Policy::none())
///     .dns_resolver(MetadataResolver::new())
///     .build();
///
/// assert!(client.is_ok());
/// ```
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct MetadataResolver;

impl MetadataResolver {
    /// A resolver that asks the operating system and filters its answer.
    pub const fn new() -> Self {
        Self
    }
}

impl Resolve for MetadataResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let kept = answered(name.as_str(), |host| async move {
                tokio::net::lookup_host((host, 0))
                    .await
                    .map(Iterator::collect)
            })
            .await?;
            Ok(Box::new(kept.into_iter()) as Addrs)
        })
    }
}

/// The addresses `host` may be reached at: the literal itself, or what
/// `lookup` answers for a name, without the ones [`is_address`] refuses.
async fn answered<Answer>(
    host: &str,
    lookup: impl FnOnce(String) -> Answer,
) -> std::result::Result<Vec<SocketAddr>, Box<dyn std::error::Error + Send + Sync>>
where
    Answer: Future<Output = std::io::Result<Vec<SocketAddr>>>,
{
    let resolved = match literal(host) {
        Some(address) => vec![SocketAddr::new(address, 0)],
        None => lookup(host.to_string()).await?,
    };
    Ok(kept(resolved, host)?)
}

/// `addresses` without the ones [`is_address`] refuses, or an error when
/// nothing else is left to connect to.
fn kept(addresses: impl IntoIterator<Item = SocketAddr>, host: &str) -> Result<Vec<SocketAddr>> {
    let kept: Vec<SocketAddr> = addresses
        .into_iter()
        .filter(|address| !is_address(address.ip()))
        .collect();
    if kept.is_empty() {
        return Err(Error::metadata_resolution(host));
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Client;
    use reqwest::redirect::Policy;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::any;

    fn address(raw: &str) -> SocketAddr {
        raw.parse().expect("a socket address")
    }

    fn name(raw: &str) -> Name {
        raw.parse().expect("a name")
    }

    #[test]
    fn a_name_resolving_only_to_link_local_addresses_is_refused() {
        let resolved = [address("169.254.169.254:0"), address("[fe80::1]:0")];

        let refused = kept(resolved, "rebound.example")
            .expect_err("a name answering only with link-local addresses must not be reached");

        assert_eq!(
            refused.to_string(),
            "rebound.example resolves only to link-local or cloud metadata addresses"
        );
        assert!(
            matches!(&refused, Error::MetadataResolution { host } if host == "rebound.example"),
            "{refused:?}"
        );
    }

    #[test]
    fn a_link_local_address_beside_a_reachable_one_is_dropped() {
        let reachable = address("192.168.1.20:0");
        let resolved = [address("169.254.169.254:0"), reachable];

        assert_eq!(
            kept(resolved, "mixed.example").expect("the LAN address stands"),
            vec![reachable]
        );
    }

    #[tokio::test]
    async fn the_resolver_keeps_loopback() {
        let resolved = MetadataResolver::new().resolve(name("localhost")).await;

        assert!(resolved.is_ok_and(|mut addresses| {
            addresses
                .next()
                .is_some_and(|address| address.ip().is_loopback())
        }));
    }

    #[tokio::test]
    async fn the_resolver_refuses_a_metadata_literal_without_looking_it_up() {
        for literal in [
            "169.254.169.254",
            "[fd00:ec2::254]",
            "::ffff:100.100.100.200",
        ] {
            let error = MetadataResolver::new()
                .resolve(name(literal))
                .await
                .err()
                .expect("a metadata literal must not be reached");

            assert_eq!(
                error.to_string(),
                format!("{literal} resolves only to link-local or cloud metadata addresses")
            );
        }
    }

    #[tokio::test]
    async fn a_name_answered_only_with_metadata_addresses_is_refused() {
        let error = answered("rebound.example", |_| async {
            Ok(vec![
                address("169.254.169.254:0"),
                address("[fd00:ec2::254]:0"),
            ])
        })
        .await
        .expect_err("a name rebound to metadata must not be reached");

        assert_eq!(
            error.to_string(),
            "rebound.example resolves only to link-local or cloud metadata addresses"
        );
    }

    #[tokio::test]
    async fn a_name_keeps_every_answer_but_the_metadata_ones() {
        let kept = answered("mixed.example", |_| async {
            Ok(vec![address("169.254.169.254:0"), address("10.0.0.5:0")])
        })
        .await
        .expect("a LAN answer is reachable");

        assert_eq!(kept, vec![address("10.0.0.5:0")]);
    }

    #[tokio::test]
    async fn a_failed_lookup_is_reported_as_it_failed() {
        let error = answered("missing.example", |_| async {
            Err(std::io::Error::other("no such host"))
        })
        .await
        .expect_err("the lookup failed");

        assert_eq!(error.to_string(), "no such host");
    }

    #[tokio::test]
    async fn the_resolver_answers_a_reachable_literal_with_itself() {
        for (literal, expected) in [("10.0.0.5", "10.0.0.5:0"), ("[::1]", "[::1]:0")] {
            let addresses: Vec<SocketAddr> = MetadataResolver::new()
                .resolve(name(literal))
                .await
                .expect("a LAN or loopback literal is reachable")
                .collect();

            assert_eq!(addresses, vec![address(expected)], "{literal}");
        }
    }

    #[tokio::test]
    async fn a_client_of_your_own_reaches_loopback_by_name_through_the_resolver() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .dns_resolver(MetadataResolver::new())
            .build()
            .expect("a client");
        let localhost = server.uri().replace("127.0.0.1", "localhost");

        let answered = client.post(&localhost).send().await;

        assert!(
            answered.is_ok_and(|response| response.status().is_success()),
            "{localhost}"
        );
        server.verify().await;
    }
}
