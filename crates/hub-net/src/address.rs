//! The address classes of the destination policy (§6.5, §3.9): which
//! addresses a request Hub performs may connect to.
//!
//! A device's `net.http` reaches Hub from the LAN, unauthenticated after the
//! handshake (§3.9), so without this check anyone who can speak to Hub's
//! port, or an applet, could use Hub to reach the owner's router, NAS,
//! printer and Hub's own loopback services. Every address a request
//! actually connects to is classified here, and anything that is not a
//! public unicast address is refused with `invalid_request`.
//!
//! The classes are those of the IANA special-purpose registries (IPv4 and
//! IPv6) that are not globally reachable, plus multicast. IPv6 outside
//! 2000::/3 is unassigned or special and refused whole; addresses that embed
//! an IPv4 address (IPv4-mapped, the NAT64 well-known prefix, 6to4) are
//! judged by that IPv4 address, so `::ffff:127.0.0.1` is loopback.
//!
//! **One block of those registries is let through: the benchmarking ranges**
//! (198.18.0.0/15 of RFC 2544 and 2001:2::/48), as the reference
//! implementation lets them through. Proxy tools in "fake IP" mode answer
//! every name from 198.18.0.0/15 and map the connection to its real
//! destination themselves; refusing the block would refuse every request on a
//! computer that runs one, and on any other computer the block reaches nothing.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Why an address is not a public destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressClass {
    /// 0.0.0.0/8 ("this network"), `::`.
    Unspecified,
    /// 127.0.0.0/8, `::1`.
    Loopback,
    /// RFC 1918: 10/8, 172.16/12, 192.168/16; and the deprecated IPv6 site-local fec0::/10.
    Private,
    /// 100.64.0.0/10, carrier-grade NAT (RFC 6598).
    SharedCgnat,
    /// 169.254.0.0/16, fe80::/10.
    LinkLocal,
    /// 224.0.0.0/4, ff00::/8.
    Multicast,
    /// fc00::/7 (RFC 4193).
    UniqueLocal,
    /// 192.0.2/24, 198.51.100/24, 203.0.113/24, 2001:db8::/32, 3fff::/20.
    Documentation,
    /// 255.255.255.255.
    Broadcast,
    /// Everything else special: 192.0.0/24, 192.88.99/24, 240/4, IPv4-compatible
    /// ::/96, 64:ff9b:1::/48, 100::/64, Teredo 2001::/32, ORCHID 2001:10::/28,
    /// and IPv6 outside 2000::/3.
    Reserved,
}

impl AddressClass {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Loopback => "loopback",
            Self::Private => "private",
            Self::SharedCgnat => "shared (CGNAT)",
            Self::LinkLocal => "link-local",
            Self::Multicast => "multicast",
            Self::UniqueLocal => "unique-local",
            Self::Documentation => "documentation",
            Self::Broadcast => "broadcast",
            Self::Reserved => "reserved",
        }
    }
}

impl fmt::Display for AddressClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// None for a public unicast address; otherwise its class.
pub fn classify(address: IpAddr) -> Option<AddressClass> {
    match address {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

const V4: &[([u8; 4], u8, AddressClass)] = &[
    ([0, 0, 0, 0], 8, AddressClass::Unspecified),
    ([10, 0, 0, 0], 8, AddressClass::Private),
    ([100, 64, 0, 0], 10, AddressClass::SharedCgnat),
    ([127, 0, 0, 0], 8, AddressClass::Loopback),
    ([169, 254, 0, 0], 16, AddressClass::LinkLocal),
    ([172, 16, 0, 0], 12, AddressClass::Private),
    ([192, 0, 0, 0], 24, AddressClass::Reserved),
    ([192, 0, 2, 0], 24, AddressClass::Documentation),
    ([192, 88, 99, 0], 24, AddressClass::Reserved),
    ([192, 168, 0, 0], 16, AddressClass::Private),
    ([198, 51, 100, 0], 24, AddressClass::Documentation),
    ([203, 0, 113, 0], 24, AddressClass::Documentation),
    ([224, 0, 0, 0], 4, AddressClass::Multicast),
    ([255, 255, 255, 255], 32, AddressClass::Broadcast),
    ([240, 0, 0, 0], 4, AddressClass::Reserved),
];

fn classify_v4(address: Ipv4Addr) -> Option<AddressClass> {
    let value = u32::from(address);
    V4.iter().find(|(base, bits, _)| {
        let mask = if *bits == 0 { 0 } else { u32::MAX << (32 - *bits as u32) };
        value & mask == u32::from_be_bytes(*base) & mask
    })
    .map(|(_, _, class)| *class)
}

fn classify_v6(address: Ipv6Addr) -> Option<AddressClass> {
    let s = address.segments();
    let bits = u128::from(address);
    let within = |base: u128, prefix: u32| bits >> (128 - prefix) == base >> (128 - prefix);
    let embedded = |high: u16, low: u16| Ipv4Addr::from(((high as u32) << 16) | low as u32);
    if bits == 0 {
        return Some(AddressClass::Unspecified);
    }
    if bits == 1 {
        return Some(AddressClass::Loopback);
    }
    // ::ffff:a.b.c.d, IPv4-mapped: the connection goes to a.b.c.d.
    if within(0xffff_0000_0000, 96) {
        return classify_v4(embedded(s[6], s[7]));
    }
    // ::a.b.c.d, IPv4-compatible (deprecated).
    if within(0, 96) {
        return Some(AddressClass::Reserved);
    }
    // 64:ff9b::/96, the NAT64 well-known prefix: the translator connects to a.b.c.d.
    if within(0x0064_ff9b_u128 << 96, 96) {
        return classify_v4(embedded(s[6], s[7]));
    }
    if within(0x0064_ff9b_0001_u128 << 80, 48) || within(0x0100_u128 << 112, 64) {
        return Some(AddressClass::Reserved);
    }
    if within(0xfc00_u128 << 112, 7) {
        return Some(AddressClass::UniqueLocal);
    }
    if within(0xfe80_u128 << 112, 10) {
        return Some(AddressClass::LinkLocal);
    }
    if within(0xfec0_u128 << 112, 10) {
        return Some(AddressClass::Private);
    }
    if within(0xff00_u128 << 112, 8) {
        return Some(AddressClass::Multicast);
    }
    // Global unicast is 2000::/3; the rest is unassigned or special.
    if !within(0x2000_u128 << 112, 3) {
        return Some(AddressClass::Reserved);
    }
    if within(0x2001_0db8_u128 << 96, 32) || within(0x3fff_u128 << 112, 20) {
        return Some(AddressClass::Documentation);
    }
    // Teredo 2001::/32 and ORCHID 2001:10::/28.
    if within(0x2001_0000_u128 << 96, 32) || within(0x2001_0010_u128 << 96, 28) {
        return Some(AddressClass::Reserved);
    }
    // 2002::/16, 6to4: the relay connects to the IPv4 address in bits 16 to 47.
    if within(0x2002_u128 << 112, 16) {
        return classify_v4(embedded(s[1], s[2]));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use AddressClass::*;

    #[test]
    fn every_class_in_both_families() {
        let table: &[(&str, Option<AddressClass>)] = &[
            // IPv4
            ("0.0.0.0", Some(Unspecified)),
            ("0.1.2.3", Some(Unspecified)),
            ("10.0.0.1", Some(Private)),
            ("10.255.255.255", Some(Private)),
            ("100.63.255.255", None),
            ("100.64.0.1", Some(SharedCgnat)),
            ("100.127.255.255", Some(SharedCgnat)),
            ("100.128.0.0", None),
            ("127.0.0.1", Some(Loopback)),
            ("127.255.0.9", Some(Loopback)),
            ("169.254.169.254", Some(LinkLocal)),
            ("172.15.255.255", None),
            ("172.16.0.1", Some(Private)),
            ("172.31.255.255", Some(Private)),
            ("172.32.0.0", None),
            ("192.0.0.8", Some(Reserved)),
            ("192.0.2.1", Some(Documentation)),
            ("192.88.99.1", Some(Reserved)),
            ("192.168.1.1", Some(Private)),
            ("198.17.255.255", None),
            // The benchmarking block (RFC 2544) is public to Hub, as in the
            // reference: proxy tools in "fake IP" mode answer every name from
            // it, and refusing it would refuse every request on such a computer.
            ("198.18.0.5", None),
            ("198.19.255.255", None),
            ("198.20.0.0", None),
            ("198.51.100.7", Some(Documentation)),
            ("203.0.113.9", Some(Documentation)),
            ("224.0.0.251", Some(Multicast)),
            ("239.255.255.250", Some(Multicast)),
            ("240.0.0.1", Some(Reserved)),
            ("255.255.255.254", Some(Reserved)),
            ("255.255.255.255", Some(Broadcast)),
            ("1.1.1.1", None),
            ("8.8.8.8", None),
            ("93.184.215.14", None),
            ("223.255.255.255", None),
            // IPv6
            ("::", Some(Unspecified)),
            ("::1", Some(Loopback)),
            ("::ffff:127.0.0.1", Some(Loopback)),
            ("::ffff:10.1.2.3", Some(Private)),
            ("::ffff:192.168.0.1", Some(Private)),
            ("::ffff:169.254.1.1", Some(LinkLocal)),
            ("::ffff:8.8.8.8", None),
            ("::127.0.0.1", Some(Reserved)),
            ("::8.8.8.8", Some(Reserved)),
            ("::ffff:0:8.8.8.8", Some(Reserved)),
            ("64:ff9b::7f00:1", Some(Loopback)),
            ("64:ff9b::a00:1", Some(Private)),
            ("64:ff9b::808:808", None),
            ("64:ff9b:1::1", Some(Reserved)),
            ("100::1", Some(Reserved)),
            ("fc00::1", Some(UniqueLocal)),
            ("fd12:3456:789a::1", Some(UniqueLocal)),
            ("fe80::1", Some(LinkLocal)),
            ("febf::1", Some(LinkLocal)),
            ("fec0::1", Some(Private)),
            ("ff02::1", Some(Multicast)),
            ("ff0e::fb", Some(Multicast)),
            ("2001:db8::1", Some(Documentation)),
            ("3fff::1", Some(Documentation)),
            ("2001:2::1", None),
            ("2001::1", Some(Reserved)),
            ("2001:10::1", Some(Reserved)),
            ("2002:7f00:1::1", Some(Loopback)),
            ("2002:c0a8:101::1", Some(Private)),
            ("2002:808:808::1", None),
            ("4000::1", Some(Reserved)),
            ("1::1", Some(Reserved)),
            ("2606:4700:4700::1111", None),
            ("2001:4860:4860::8888", None),
            ("2a00:1450:4001:80b::200e", None),
        ];
        for (text, expected) in table {
            let address: IpAddr = text.parse().unwrap();
            assert_eq!(classify(address), *expected, "{text}");
        }
    }
}
