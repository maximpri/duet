// SPDX-License-Identifier: GPL-3.0-or-later
//! Which addresses a fetch may connect to.
//!
//! A fetch is the frontier asking the host to open a connection, so it must
//! never reach what only the host can reach: loopback services, the local
//! network, cloud metadata endpoints. Addresses are checked after name
//! resolution (a public name can point anywhere) and the connection is then
//! made to exactly the checked address, so a second lookup cannot answer
//! differently. Owner-listed intranet hosts and networks are allowed; cloud
//! metadata endpoints never are.

use ipnet::IpNet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Cloud instance metadata endpoints: credentials live there. Refused even
/// when allowlisted.
const METADATA_ADDRS: &[IpAddr] = &[
    IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
    IpAddr::V4(Ipv4Addr::new(169, 254, 170, 2)),
    IpAddr::V4(Ipv4Addr::new(169, 254, 169, 123)),
    IpAddr::V4(Ipv4Addr::new(100, 100, 100, 200)),
    IpAddr::V6(Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254)),
];

/// Names of metadata services, refused before any lookup.
const METADATA_NAMES: &[&str] = &[
    "metadata",
    "metadata.google.internal",
    "metadata.goog",
    "instance-data",
    "instance-data.ec2.internal",
];

/// Hosts and networks the owner allows despite being private
/// (`web.allowlist_private`).
#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    names: Vec<String>,
    /// `*.corp.example` entries, stored as `.corp.example`.
    suffixes: Vec<String>,
    nets: Vec<IpNet>,
}

impl Allowlist {
    /// Parses entries: a host name, `*.domain`, an IP address or a CIDR network.
    pub fn parse(entries: &[String]) -> Result<Self, String> {
        let mut out = Allowlist::default();
        for raw in entries {
            let e = raw.trim().to_ascii_lowercase();
            if e.is_empty() {
                continue;
            }
            if let Ok(net) = e.parse::<IpNet>() {
                out.nets.push(net);
            } else if let Ok(ip) = e.parse::<IpAddr>() {
                out.nets.push(IpNet::from(ip));
            } else if let Some(domain) = e.strip_prefix("*.") {
                if domain.is_empty() || domain.contains(['/', ':', '*']) {
                    return Err(format!("{raw:?} is not a host, network or *.domain"));
                }
                out.suffixes.push(format!(".{domain}"));
            } else if e.contains(['/', '*', ' ']) {
                return Err(format!("{raw:?} is not a host, network or *.domain"));
            } else {
                out.names.push(e.trim_end_matches('.').to_owned());
            }
        }
        Ok(out)
    }

    /// Whether the owner listed this host name.
    pub fn has_name(&self, host: &str) -> bool {
        let h = host.trim_end_matches('.').to_ascii_lowercase();
        self.names.contains(&h) || self.suffixes.iter().any(|s| h.ends_with(s.as_str()))
    }

    fn has_addr(&self, ip: IpAddr) -> bool {
        self.nets.iter().any(|n| n.contains(&ip))
    }
}

/// Whether `host` names a metadata service.
pub fn is_metadata_name(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    METADATA_NAMES.contains(&h.as_str())
}

/// Why `ip` may not be fetched from (`None`: it may). `named_ok` is true when
/// the host name it was resolved from is allowlisted.
pub fn refusal(ip: IpAddr, allow: &Allowlist, named_ok: bool) -> Option<&'static str> {
    let ip = canonical(ip);
    if METADATA_ADDRS.contains(&ip) {
        return Some("a cloud metadata address");
    }
    let class = class_of(ip)?;
    if named_ok || allow.has_addr(ip) {
        return None;
    }
    Some(class)
}

/// The IPv4 address inside an IPv4-mapped, IPv4-compatible, NAT64 or 6to4
/// IPv6 address, so `::ffff:127.0.0.1` is judged as `127.0.0.1`.
fn canonical(ip: IpAddr) -> IpAddr {
    let IpAddr::V6(v6) = ip else { return ip };
    if let Some(v4) = v6.to_ipv4_mapped() {
        return IpAddr::V4(v4);
    }
    let s = v6.segments();
    let embedded = |hi: u16, lo: u16| {
        IpAddr::V4(Ipv4Addr::new(
            (hi >> 8) as u8,
            hi as u8,
            (lo >> 8) as u8,
            lo as u8,
        ))
    };
    match s {
        // IPv4-compatible (deprecated) `::a.b.c.d`, but not `::` or `::1`.
        [0, 0, 0, 0, 0, 0, hi, lo] if hi != 0 => embedded(hi, lo),
        // NAT64 well-known prefix 64:ff9b::/96.
        [0x64, 0xff9b, 0, 0, 0, 0, hi, lo] => embedded(hi, lo),
        // 6to4 2002:a.b.c.d::/48.
        [0x2002, hi, lo, ..] => embedded(hi, lo),
        _ => ip,
    }
}

/// The non-public class of an address, `None` for a public one.
fn class_of(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            if v4.is_unspecified() || a == 0 {
                Some("an unspecified address")
            } else if v4.is_loopback() {
                Some("a loopback address")
            } else if v4.is_private() {
                Some("a private address")
            } else if v4.is_link_local() {
                Some("a link-local address")
            } else if a == 100 && (64..128).contains(&b) {
                Some("a carrier-grade NAT address")
            } else if v4.is_broadcast() || v4.is_multicast() || a >= 240 {
                Some("a broadcast, multicast or reserved address")
            } else if a == 192 && b == 0 && c == 0 {
                Some("an IETF protocol address")
            } else if a == 198 && (b == 18 || b == 19) {
                Some("a benchmarking address")
            } else if v4.is_documentation() {
                Some("a documentation address")
            } else {
                None
            }
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            if v6.is_unspecified() {
                Some("an unspecified address")
            } else if v6.is_loopback() {
                Some("a loopback address")
            } else if s[0] & 0xfe00 == 0xfc00 {
                Some("a unique-local address")
            } else if s[0] & 0xffc0 == 0xfe80 || s[0] & 0xffc0 == 0xfec0 {
                Some("a link-local address")
            } else if s[0] & 0xff00 == 0xff00 {
                Some("a multicast address")
            } else if s[0] == 0x2001 && s[1] == 0x0db8 {
                Some("a documentation address")
            } else if s[0] == 0x2001 && s[1] == 0 {
                // Teredo tunnels carry an obfuscated IPv4 address.
                Some("a Teredo address")
            } else if s[0] == 0x0100 && s[1..4] == [0, 0, 0] {
                Some("a discard-only address")
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(s: &str) -> bool {
        refusal(s.parse().unwrap(), &Allowlist::default(), false).is_some()
    }

    #[test]
    fn private_loopback_link_local_cgnat_and_metadata_are_refused() {
        for a in [
            "127.0.0.1",
            "127.8.9.1",
            "0.0.0.0",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "169.254.1.1",
            "100.64.0.1",
            "100.100.100.200",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fd00::1",
            "fd00:ec2::254",
            "fc00::5",
            "fe80::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::127.0.0.1",
            "64:ff9b::a9fe:a9fe",
            "2002:c0a8:0101::1",
            "2001:0:4136:e378::1",
        ] {
            assert!(refused(a), "{a} was allowed");
        }
        for a in [
            "1.1.1.1",
            "93.184.216.34",
            "172.32.0.1",
            "100.128.0.1",
            "2606:4700::1111",
            "2002:0808:0808::1",
        ] {
            assert!(!refused(a), "{a} was refused");
        }
    }

    #[test]
    fn the_allowlist_opens_private_hosts_but_never_metadata() {
        let allow = Allowlist::parse(&[
            "wiki.corp".into(),
            "*.intra.example".into(),
            "10.20.0.0/16".into(),
            "169.254.0.0/16".into(),
        ])
        .unwrap();
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(refusal(ip("10.20.3.4"), &allow, false).is_none());
        assert!(refusal(ip("10.21.3.4"), &allow, false).is_some());
        assert!(allow.has_name("wiki.corp") && allow.has_name("WIKI.corp."));
        assert!(allow.has_name("docs.intra.example"));
        assert!(!allow.has_name("intra.example.evil.com"));
        assert!(refusal(ip("192.168.0.9"), &allow, true).is_none());
        assert!(refusal(ip("169.254.169.254"), &allow, true).is_some());
        assert!(refusal(ip("169.254.1.1"), &allow, false).is_none());
        assert!(is_metadata_name("metadata.google.internal."));
        assert!(Allowlist::parse(&["a b".into()]).is_err());
    }
}
