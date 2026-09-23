use std::net::Ipv4Addr;

const BITS: u32 = Ipv4Addr::BITS;

/// An IPv4 network written as an address and a prefix length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ipv4Block {
    network: u32,
    mask: u32,
}

impl Ipv4Block {
    /// `0.0.0.0/8`: "this network", never a destination.
    pub(crate) const THIS_NETWORK: Self = Self::new(Ipv4Addr::new(0, 0, 0, 0), 8);
    /// `100.64.0.0/10`: shared address space, routed inside carrier and cloud
    /// networks.
    pub(crate) const SHARED_ADDRESS_SPACE: Self = Self::new(Ipv4Addr::new(100, 64, 0, 0), 10);
    /// `192.0.0.0/24`: IETF protocol assignments.
    pub(crate) const PROTOCOL_ASSIGNMENTS: Self = Self::new(Ipv4Addr::new(192, 0, 0, 0), 24);
    /// `192.0.2.0/24`: TEST-NET-1, reserved for documentation.
    pub(crate) const TEST_NET_1: Self = Self::new(Ipv4Addr::new(192, 0, 2, 0), 24);
    /// `192.88.99.0/24`: the deprecated 6to4 relay anycast block.
    pub(crate) const SIX_TO_FOUR_RELAY: Self = Self::new(Ipv4Addr::new(192, 88, 99, 0), 24);
    /// `198.18.0.0/15`: reserved for benchmarking.
    pub(crate) const BENCHMARKING: Self = Self::new(Ipv4Addr::new(198, 18, 0, 0), 15);
    /// `198.51.100.0/24`: TEST-NET-2, reserved for documentation.
    pub(crate) const TEST_NET_2: Self = Self::new(Ipv4Addr::new(198, 51, 100, 0), 24);
    /// `203.0.113.0/24`: TEST-NET-3, reserved for documentation.
    pub(crate) const TEST_NET_3: Self = Self::new(Ipv4Addr::new(203, 0, 113, 0), 24);
    /// `240.0.0.0/4`: reserved, including the limited broadcast address.
    pub(crate) const RESERVED: Self = Self::new(Ipv4Addr::new(240, 0, 0, 0), 4);

    pub(crate) const fn new(network: Ipv4Addr, prefix: u32) -> Self {
        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (BITS - prefix)
        };
        Self {
            network: network.to_bits() & mask,
            mask,
        }
    }

    pub(crate) const fn contains(self, ip: Ipv4Addr) -> bool {
        ip.to_bits() & self.mask == self.network
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_holds_its_first_and_last_address_and_nothing_either_side() {
        let block = Ipv4Block::SHARED_ADDRESS_SPACE;

        assert!(block.contains(Ipv4Addr::new(100, 64, 0, 0)));
        assert!(block.contains(Ipv4Addr::new(100, 127, 255, 255)));
        assert!(!block.contains(Ipv4Addr::new(100, 63, 255, 255)));
        assert!(!block.contains(Ipv4Addr::new(100, 128, 0, 0)));
    }

    #[test]
    fn a_zero_prefix_holds_every_address() {
        let block = Ipv4Block::new(Ipv4Addr::new(10, 0, 0, 0), 0);

        assert!(block.contains(Ipv4Addr::new(0, 0, 0, 0)));
        assert!(block.contains(Ipv4Addr::new(255, 255, 255, 255)));
    }

    #[test]
    fn a_full_prefix_holds_one_address() {
        let block = Ipv4Block::new(Ipv4Addr::new(10, 0, 0, 1), BITS);

        assert!(block.contains(Ipv4Addr::new(10, 0, 0, 1)));
        assert!(!block.contains(Ipv4Addr::new(10, 0, 0, 2)));
    }
}
