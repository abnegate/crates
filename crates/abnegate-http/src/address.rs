mod ipv4_block;
mod ipv6_block;

use crate::address::ipv4_block::Ipv4Block;
use crate::address::ipv6_block::Ipv6Block;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;

/// The IPv4 blocks outside the private, loopback, link-local, multicast and
/// broadcast ranges the standard library already names.
const REFUSED_IPV4: [Ipv4Block; 9] = [
    Ipv4Block::THIS_NETWORK,
    Ipv4Block::SHARED_ADDRESS_SPACE,
    Ipv4Block::PROTOCOL_ASSIGNMENTS,
    Ipv4Block::TEST_NET_1,
    Ipv4Block::SIX_TO_FOUR_RELAY,
    Ipv4Block::BENCHMARKING,
    Ipv4Block::TEST_NET_2,
    Ipv4Block::TEST_NET_3,
    Ipv4Block::RESERVED,
];

/// The IPv6 blocks outside the loopback, unspecified, unique-local,
/// link-local and multicast ranges the standard library already names.
const REFUSED_IPV6: [Ipv6Block; 7] = [
    Ipv6Block::NAT64_LOCAL,
    Ipv6Block::DISCARD_ONLY,
    Ipv6Block::PROTOCOL_ASSIGNMENTS,
    Ipv6Block::DOCUMENTATION,
    Ipv6Block::SIX_TO_FOUR,
    Ipv6Block::EXPANDED_DOCUMENTATION,
    Ipv6Block::SITE_LOCAL,
];

/// The address `host` spells, if it is an IP literal rather than a name.
///
/// URL hosts and resolver names both carry IPv6 literals in brackets, which
/// do not parse as an address until they are removed.
pub(crate) fn literal(host: &str) -> Option<IpAddr> {
    host.strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
        .parse()
        .ok()
}

/// Whether `ip` is an address a caller-supplied fetch must not reach.
///
/// `Ipv4Addr::is_global` would answer this, but it is still unstable, so the
/// non-global ranges are named here. Enumerating them is the whole point: the
/// obvious three private blocks leave shared address space and benchmarking
/// space reachable, and those are internal destinations like any other.
pub(crate) fn must_not_be_fetched(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ipv4_must_not_be_fetched(ip),
        IpAddr::V6(ip) => match embedded_ipv4(ip) {
            Some(ip) => ipv4_must_not_be_fetched(ip),
            None => ipv6_must_not_be_fetched(ip),
        },
    }
}

fn ipv4_must_not_be_fetched(ip: Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || REFUSED_IPV4.iter().any(|block| block.contains(ip))
}

fn ipv6_must_not_be_fetched(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || ip.is_multicast()
        || REFUSED_IPV6.iter().any(|block| block.contains(ip))
}

/// The IPv4 address an IPv6 address reaches, so it is answered by the IPv4
/// rules rather than a second, weaker set.
///
/// A NAT64 gateway forwards the well-known prefix to the IPv4 address in its
/// last 32 bits, so that address is what decides; refusing the whole prefix
/// would cut an IPv6-only host off from every IPv4-only site.
fn embedded_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    if Ipv6Block::NAT64.contains(ip) {
        let [.., first, second, third, fourth] = ip.octets();
        return Some(Ipv4Addr::new(first, second, third, fourth));
    }
    ip.to_ipv4_mapped().or_else(|| ip.to_ipv4())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(raw: &str) -> IpAddr {
        raw.parse().expect("an address")
    }

    #[test]
    fn the_non_global_ranges_beyond_the_private_ones_are_refused() {
        for raw in [
            "0.0.0.0",
            "0.255.255.255",
            "100.64.0.1",
            "100.127.255.1",
            "198.18.0.1",
            "198.19.255.1",
            "192.0.0.1",
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "192.88.99.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
        ] {
            assert!(must_not_be_fetched(ip(raw)), "{raw} is reachable");
        }
    }

    #[test]
    fn a_globally_routable_address_is_still_reachable() {
        for raw in [
            "93.184.216.34",
            "1.1.1.1",
            "100.63.255.255",
            "100.128.0.0",
            "198.17.255.255",
            "198.20.0.0",
            "2606:2800:220:1:248:1893:25c8:1946",
        ] {
            assert!(!must_not_be_fetched(ip(raw)), "{raw} was refused");
        }
    }

    #[test]
    fn the_ipv6_ranges_that_never_leave_a_site_are_refused() {
        for raw in [
            "::",
            "::1",
            "fd00::1",
            "fe80::1",
            "ff02::1",
            "100::1",
            "2001::1",
            "2001:db8::1",
            "3fff::1",
        ] {
            assert!(must_not_be_fetched(ip(raw)), "{raw} is reachable");
        }
    }

    #[test]
    fn nat64_six_to_four_and_site_local_addresses_are_refused() {
        for raw in [
            "64:ff9b::7f00:1",
            "64:ff9b::a00:1",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b:1::5db8:d822",
            "2002:5db8:d822::1",
            "2002:7f00:1::",
            "fec0::1",
            "feff::1",
        ] {
            assert!(must_not_be_fetched(ip(raw)), "{raw} is reachable");
        }
    }

    #[test]
    fn a_nat64_address_embedding_a_public_ipv4_address_is_reachable() {
        assert!(!must_not_be_fetched(ip("64:ff9b::5db8:d822")));
    }

    #[test]
    fn an_ipv4_address_written_as_ipv6_answers_to_the_ipv4_rules() {
        assert!(must_not_be_fetched(ip("::ffff:127.0.0.1")));
        assert!(must_not_be_fetched(ip("::ffff:169.254.169.254")));
        assert!(must_not_be_fetched(ip("::10.0.0.1")));
        assert!(!must_not_be_fetched(ip("::ffff:93.184.216.34")));
    }

    #[test]
    fn a_literal_is_read_with_or_without_brackets() {
        assert_eq!(literal("127.0.0.1"), Some(ip("127.0.0.1")));
        assert_eq!(literal("[::1]"), Some(ip("::1")));
        assert_eq!(literal("::1"), Some(ip("::1")));
        assert_eq!(literal("example.com"), None);
        assert_eq!(literal("[example.com]"), None);
    }
}
