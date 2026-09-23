use std::net::Ipv6Addr;

const BITS: u32 = Ipv6Addr::BITS;

/// An IPv6 network written as an address and a prefix length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ipv6Block {
    network: u128,
    mask: u128,
}

impl Ipv6Block {
    /// `64:ff9b::/96`: the well-known NAT64 prefix, which carries an IPv4
    /// address in its last 32 bits.
    pub(crate) const NAT64: Self = Self::new(Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0, 0), 96);
    /// `64:ff9b:1::/48`: the NAT64 prefix reserved for local use.
    pub(crate) const NAT64_LOCAL: Self =
        Self::new(Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0, 0), 48);
    /// `100::/64`: discard-only.
    pub(crate) const DISCARD_ONLY: Self = Self::new(Ipv6Addr::new(0x100, 0, 0, 0, 0, 0, 0, 0), 64);
    /// `2001::/23`: IETF protocol assignments, Teredo among them.
    pub(crate) const PROTOCOL_ASSIGNMENTS: Self =
        Self::new(Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23);
    /// `2001:db8::/32`: reserved for documentation.
    pub(crate) const DOCUMENTATION: Self =
        Self::new(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0), 32);
    /// `2002::/16`: 6to4, which tunnels to whatever IPv4 address it embeds.
    pub(crate) const SIX_TO_FOUR: Self = Self::new(Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16);
    /// `3ff0::/12`: unallocated space around the `3fff::/20` documentation
    /// block.
    pub(crate) const EXPANDED_DOCUMENTATION: Self =
        Self::new(Ipv6Addr::new(0x3ff0, 0, 0, 0, 0, 0, 0, 0), 12);
    /// `fec0::/10`: deprecated site-local unicast, still routed inside some
    /// networks.
    pub(crate) const SITE_LOCAL: Self = Self::new(Ipv6Addr::new(0xfec0, 0, 0, 0, 0, 0, 0, 0), 10);

    pub(crate) const fn new(network: Ipv6Addr, prefix: u32) -> Self {
        let mask = if prefix == 0 {
            0
        } else {
            u128::MAX << (BITS - prefix)
        };
        Self {
            network: network.to_bits() & mask,
            mask,
        }
    }

    pub(crate) const fn contains(self, ip: Ipv6Addr) -> bool {
        ip.to_bits() & self.mask == self.network
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_holds_its_first_and_last_address_and_nothing_either_side() {
        let block = Ipv6Block::SITE_LOCAL;

        assert!(block.contains("fec0::".parse().expect("an address")));
        assert!(
            block.contains(
                "feff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"
                    .parse()
                    .expect("an address")
            )
        );
        assert!(
            !block.contains(
                "febf:ffff:ffff:ffff:ffff:ffff:ffff:ffff"
                    .parse()
                    .expect("an address")
            )
        );
        assert!(!block.contains("ff00::".parse().expect("an address")));
    }

    #[test]
    fn the_nat64_prefix_ends_where_its_embedded_address_begins() {
        assert!(Ipv6Block::NAT64.contains("64:ff9b::ffff:ffff".parse().expect("an address")));
        assert!(!Ipv6Block::NAT64.contains("64:ff9b::1:0:0".parse().expect("an address")));
        assert!(!Ipv6Block::NAT64.contains("64:ff9b:1::".parse().expect("an address")));
    }

    #[test]
    fn a_zero_prefix_holds_every_address() {
        let block = Ipv6Block::new(Ipv6Addr::LOCALHOST, 0);

        assert!(block.contains(Ipv6Addr::UNSPECIFIED));
        assert!(block.contains(Ipv6Addr::from_bits(u128::MAX)));
    }
}
