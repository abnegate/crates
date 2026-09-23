use crate::address::literal;
use crate::address::must_not_be_fetched;
use crate::error::HttpError;
use crate::error::Result;
use reqwest::dns::Addrs;
use reqwest::dns::Name;
use reqwest::dns::Resolve;
use reqwest::dns::Resolving;
use std::iter;
use std::net::SocketAddr;

/// Resolves a name and refuses the addresses
/// [`validate_public_url`](crate::validate_public_url) cannot see.
///
/// The URL check reads text; the address a name resolves to is chosen by DNS
/// afterwards, so a public hostname pointing at 127.0.0.1 or a LAN address
/// passes every textual check and is still an internal request. reqwest only
/// consults a resolver for names, never for IP literals, so the literal check
/// here backs up [`PublicClient`](crate::PublicClient) rather than replacing it.
pub(crate) struct PublicResolver;

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str();
            if let Some(ip) = literal(host) {
                if must_not_be_fetched(ip) {
                    return Err(HttpError::PrivateAddress.into());
                }
                return Ok(Box::new(iter::once(SocketAddr::new(ip, 0))) as Addrs);
            }

            let resolved = tokio::net::lookup_host((host, 0)).await?;
            let public = public_only(resolved, host)?;
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

/// Keep only the addresses a caller-supplied fetch may connect to.
///
/// A name that answers with both a public and a private address is not
/// refused outright: the private one is dropped, so a connector that falls
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

#[cfg(test)]
mod tests {
    use super::*;

    fn address(raw: &str) -> SocketAddr {
        raw.parse().expect("a socket address")
    }

    fn name(raw: &str) -> Name {
        raw.parse().expect("a name")
    }

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
    async fn the_resolver_refuses_a_name_that_answers_with_loopback() {
        let error = Resolve::resolve(&PublicResolver, name("localhost"))
            .await
            .err()
            .expect("localhost must not be fetchable");

        assert!(error.to_string().contains("must not be fetched"), "{error}");
    }

    #[tokio::test]
    async fn the_resolver_refuses_a_private_literal_without_looking_it_up() {
        for literal in ["127.0.0.1", "::ffff:127.0.0.1", "[::1]", "169.254.169.254"] {
            let error = Resolve::resolve(&PublicResolver, name(literal))
                .await
                .err()
                .expect("a private literal must not be fetchable");

            assert!(
                matches!(
                    error.downcast_ref::<HttpError>(),
                    Some(HttpError::PrivateAddress)
                ),
                "{literal}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn the_resolver_answers_a_public_literal_with_itself() {
        let addresses: Vec<SocketAddr> = Resolve::resolve(&PublicResolver, name("93.184.216.34"))
            .await
            .expect("a public literal is fetchable")
            .collect();

        assert_eq!(addresses, vec![address("93.184.216.34:0")]);
    }
}
