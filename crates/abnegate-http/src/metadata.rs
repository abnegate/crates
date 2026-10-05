//! The link-local and cloud metadata targets an endpoint someone configured
//! must never reach, even where the rest of the deployment is fair game.
//!
//! [`validate_public_url`](crate::validate_public_url) refuses everything
//! inside the deployment, which is right for a URL from the open internet and
//! wrong for an endpoint an operator or a tenant set up on purpose: a
//! self-hosted model server lives on loopback, the LAN or a single-label host.
//! What no such endpoint may name is the instance's own credentials, so this
//! module refuses only link-local addresses and the cloud metadata services:
//! `169.254.0.0/16`, `100.100.100.200`, `168.63.129.16`, `fe80::/10`,
//! `fd00:ec2::254`, `fd20:ce::254`, the local-use NAT64 prefix
//! `64:ff9b:1::/48`, `metadata.google.internal` and `metadata.goog`. [`MetadataResolver`] applies the same rule to every
//! address a name resolves to, and
//! [`TargetPolicy::ExceptMetadata`](crate::TargetPolicy) applies all of it to a
//! [`PublicClient`](crate::PublicClient).
//!
//! ```
//! use abnegate_http::metadata::{is_host, is_url};
//! use reqwest::Url;
//!
//! assert!(is_host("METADATA.google.internal."));
//! assert!(is_host("[fd00:ec2::254]"));
//! assert!(!is_host("localhost"));
//!
//! let url = Url::parse("http://169.254.169.254/latest/meta-data/").expect("a URL");
//! assert!(is_url(&url));
//! ```

mod resolver;

use crate::address::embedded_ipv4;
use crate::address::ipv6_block::Ipv6Block;
use crate::address::literal;
use crate::error::Error;
use crate::error::Result;
use crate::url::parse;
use reqwest::Url;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;

pub use crate::metadata::resolver::MetadataResolver;

const HOSTS: [&str; 2] = ["metadata.google.internal", "metadata.goog"];
const IPV4_ADDRESSES: [Ipv4Addr; 2] = [
    Ipv4Addr::new(100, 100, 100, 200),
    Ipv4Addr::new(168, 63, 129, 16),
];
const IPV6_ADDRESSES: [Ipv6Addr; 2] = [
    Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254),
    Ipv6Addr::new(0xfd20, 0x00ce, 0, 0, 0, 0, 0, 0x0254),
];

/// Whether `address` is link-local or a cloud provider's metadata service.
///
/// An IPv6 address that carries an IPv4 one, IPv4-mapped or through the
/// well-known NAT64 prefix, is judged as that IPv4 address. The local-use
/// NAT64 prefix, `64:ff9b:1::/48`, can carry any IPv4 address in a layout
/// only its translator knows, so all of it is refused.
pub fn is_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_ipv4_address(address),
        IpAddr::V6(address) => match embedded_ipv4(address) {
            Some(address) => is_ipv4_address(address),
            None => {
                address.is_unicast_link_local()
                    || Ipv6Block::NAT64_LOCAL.contains(address)
                    || IPV6_ADDRESSES.contains(&address)
            }
        },
    }
}

fn is_ipv4_address(address: Ipv4Addr) -> bool {
    address.is_link_local() || IPV4_ADDRESSES.contains(&address)
}

/// Whether `host` names a cloud provider's metadata service, or is a
/// link-local or metadata address written as one, in any case, with or
/// without a trailing dot or the brackets around an IPv6 literal.
pub fn is_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    match literal(&host) {
        Some(address) => is_address(address),
        None => HOSTS.contains(&host.as_str()),
    }
}

/// Whether `url` points at a host [`is_host`] refuses.
pub fn is_url(url: &Url) -> bool {
    url.host_str().is_some_and(is_host)
}

/// Parse `raw` and refuse it only if it cannot be fetched at all or names a
/// target [`is_url`] refuses.
pub(crate) fn validate(raw: &str) -> Result<Url> {
    let url = parse(raw)?;
    if is_url(&url) {
        return Err(Error::MetadataTarget);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(raw: &str) -> IpAddr {
        raw.parse().expect("an address")
    }

    #[test]
    fn link_local_and_metadata_addresses_are_refused() {
        for refused in [
            "169.254.169.254",
            "169.254.0.1",
            "fe80::1",
            "febf::1",
            "fd00:ec2::254",
            "::ffff:169.254.169.254",
            "100.100.100.200",
        ] {
            assert!(is_address(address(refused)), "{refused}");
        }
        for allowed in [
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.20",
            "172.17.0.1",
            "::1",
            "fd00::1",
            "fd00:ec2::253",
            "100.100.100.100",
            "8.8.8.8",
        ] {
            assert!(!is_address(address(allowed)), "{allowed}");
        }
    }

    #[test]
    fn the_local_use_nat64_prefix_is_refused() {
        for refused in ["64:ff9b:1::a9fe:a9fe", "64:ff9b:1:ffff::1"] {
            assert!(is_address(address(refused)), "{refused}");
            assert!(is_host(&format!("[{refused}]")), "{refused}");
        }
        assert!(!is_address(address("64:ff9b:2::a9fe:a9fe")));
    }

    #[test]
    fn gcp_ipv6_and_azure_wireserver_metadata_addresses_are_refused() {
        for refused in ["fd20:ce::254", "::ffff:168.63.129.16", "168.63.129.16"] {
            assert!(is_address(address(refused)), "{refused}");
        }
        for refused in ["[fd20:ce::254]", "168.63.129.16"] {
            assert!(is_host(refused), "{refused}");
        }
        for allowed in [
            "fd20:ce::253",
            "fd20::254",
            "168.63.129.17",
            "168.63.128.16",
        ] {
            assert!(!is_address(address(allowed)), "{allowed}");
        }
    }

    #[test]
    fn metadata_hosts_are_refused_however_they_are_spelled() {
        for refused in [
            "metadata.google.internal",
            "METADATA.google.internal.",
            "metadata.goog",
            "169.254.169.254",
            "[fe80::1]",
            "[fd00:ec2::254]",
            "[::ffff:169.254.169.254]",
        ] {
            assert!(is_host(refused), "{refused}");
        }
        for allowed in [
            "localhost",
            "litellm",
            "metadata",
            "ollama.lan",
            "gateway.internal",
            "api.openai.com",
            "[::1]",
            "192.168.1.20",
        ] {
            assert!(!is_host(allowed), "{allowed}");
        }
    }

    #[test]
    fn a_metadata_address_behind_nat64_or_ipv4_compatibility_is_refused() {
        for refused in [
            "64:ff9b::a9fe:a9fe",
            "64:ff9b::6464:64c8",
            "::169.254.169.254",
        ] {
            assert!(is_address(address(refused)), "{refused}");
        }
        for allowed in ["64:ff9b::7f00:1", "64:ff9b::5db8:d822", "::"] {
            assert!(!is_address(address(allowed)), "{allowed}");
        }
    }

    #[test]
    fn a_url_is_judged_by_its_host_alone() {
        for refused in [
            "http://169.254.169.254/latest",
            "http://2852039166/latest",
            "http://0xa9.254.0xa9.0xfe/latest",
            "http://[::ffff:169.254.169.254]/v1",
            "http://METADATA.GOOGLE.INTERNAL./v1",
        ] {
            let url = Url::parse(refused).expect("a URL");
            assert!(is_url(&url), "{refused}");
        }
        for allowed in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:4000/v1",
            "http://192.168.1.20/v1?host=169.254.169.254",
            "https://api.openai.com/v1",
        ] {
            let url = Url::parse(allowed).expect("a URL");
            assert!(!is_url(&url), "{allowed}");
        }
    }

    #[test]
    fn only_what_no_fetch_may_carry_is_refused_besides_metadata() {
        assert!(matches!(
            validate("http://169.254.170.2/v1"),
            Err(Error::MetadataTarget)
        ));
        assert!(matches!(
            validate("file:///etc/passwd"),
            Err(Error::UnsupportedScheme)
        ));
        assert!(matches!(
            validate("http://user:secret@localhost/"),
            Err(Error::EmbeddedCredentials)
        ));
        assert!(matches!(validate("not a url"), Err(Error::InvalidUrl)));
        for allowed in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:4000/v1",
            "http://10.0.0.5/v1",
            "http://litellm/v1",
            "http://[::1]:8080/v1",
        ] {
            assert!(validate(allowed).is_ok(), "{allowed}");
        }
    }
}
