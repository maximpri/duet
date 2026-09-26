// SPDX-License-Identifier: GPL-3.0-or-later
//! The hosts the proxy lets commands reach (`sandbox.registries`).
//!
//! An entry is a host name (`registry.npmjs.org`), a wildcard for the names
//! under a domain (`*.example.com`: `a.example.com`, not `example.com`), and
//! optionally a port (`nexus.example.com:8443`). Without a port, 443 and 80.
//! IP addresses are not entries: a command names the host it wants, and the
//! proxy resolves it.

use std::net::IpAddr;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pattern {
    Name(String),
    /// `*.example.com`, stored as `.example.com`.
    Under(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    pattern: Pattern,
    port: Option<u16>,
}

/// The ports an entry without one allows.
pub const DEFAULT_PORTS: [u16; 2] = [443, 80];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hosts {
    entries: Vec<Entry>,
}

impl Hosts {
    /// Parses `sandbox.registries`; an invalid entry is an error naming it.
    pub fn parse(entries: &[String]) -> Result<Self, String> {
        let mut out = Hosts::default();
        for raw in entries {
            let e = raw.trim().to_ascii_lowercase();
            if e.is_empty() {
                continue;
            }
            let bad = |why: &str| format!("{raw:?} is not a registry host: {why}");
            let (host, port) = match e.rsplit_once(':') {
                Some((h, p)) => {
                    let port: u16 = p
                        .parse()
                        .ok()
                        .filter(|p| *p != 0)
                        .ok_or_else(|| bad("the port is not a number from 1 to 65535"))?;
                    (h, Some(port))
                }
                None => (e.as_str(), None),
            };
            let host = host.trim_end_matches('.');
            let pattern = match host.strip_prefix("*.") {
                Some(domain) => {
                    if !is_name(domain) || !domain.contains('.') {
                        return Err(bad(
                            "a wildcard needs a domain of at least two labels (*.example.com)",
                        ));
                    }
                    Pattern::Under(format!(".{domain}"))
                }
                None => {
                    if host.parse::<IpAddr>().is_ok() || host.starts_with('[') {
                        return Err(bad("name the host, not an address"));
                    }
                    if !is_name(host) {
                        return Err(bad(
                            "use a host name, *.domain, and optionally :port (no scheme or path)",
                        ));
                    }
                    Pattern::Name(host.to_owned())
                }
            };
            out.entries.push(Entry { pattern, port });
        }
        Ok(out)
    }

    /// Whether a command may reach `host:port`. `host` is compared as a
    /// normalized name (lowercase, no trailing dot).
    pub fn allows(&self, host: &str, port: u16) -> bool {
        let Some(host) = normalize(host) else {
            return false;
        };
        self.entries.iter().any(|e| {
            let name_ok = match &e.pattern {
                Pattern::Name(n) => *n == host,
                Pattern::Under(suffix) => host.ends_with(suffix.as_str()),
            };
            let port_ok = match e.port {
                Some(p) => p == port,
                None => DEFAULT_PORTS.contains(&port),
            };
            name_ok && port_ok
        })
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// `host` as a comparable name: lowercase, without a trailing dot; `None`
/// when it is not a host name (an address, or characters no name has).
pub fn normalize(host: &str) -> Option<String> {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    (is_name(&h) && h.parse::<IpAddr>().is_err()).then_some(h)
}

/// A DNS host name: labels of letters, digits and inner hyphens (and `_`,
/// which some registries' CDNs use), at most 253 characters.
fn is_name(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(entries: &[&str]) -> Hosts {
        Hosts::parse(&entries.iter().map(|e| e.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn names_wildcards_and_ports() {
        let h = hosts(&[
            "registry.npmjs.org",
            "*.Example.COM",
            "nexus.corp.test:8443",
            "trailing.dot.test.",
        ]);
        assert!(h.allows("registry.npmjs.org", 443));
        assert!(h.allows("REGISTRY.npmjs.org.", 80));
        assert!(!h.allows("registry.npmjs.org", 22));
        assert!(!h.allows("evil-registry.npmjs.org", 443));
        assert!(!h.allows("registry.npmjs.org.evil.test", 443));
        assert!(h.allows("a.example.com", 443) && h.allows("a.b.example.com", 80));
        assert!(!h.allows("example.com", 443));
        assert!(!h.allows("notexample.com", 443));
        assert!(h.allows("nexus.corp.test", 8443));
        assert!(!h.allows("nexus.corp.test", 443));
        assert!(h.allows("trailing.dot.test", 443));
        // Addresses and junk never match, even when an entry could look alike.
        assert!(!h.allows("93.184.216.34", 443));
        assert!(!h.allows("[::1]", 443));
        assert!(!h.allows("registry.npmjs.org/x", 443));
        assert!(!h.allows("", 443));
    }

    #[test]
    fn bad_entries_are_named() {
        for bad in [
            "https://registry.npmjs.org",
            "registry.npmjs.org/path",
            "10.0.0.1",
            "[::1]:443",
            "*",
            "*.com",
            "*.",
            "host:0",
            "host:99999",
            "bad host",
            "-lead.example.com",
        ] {
            let err = Hosts::parse(&[bad.to_string()]).unwrap_err();
            assert!(err.contains(bad), "{bad}: {err}");
        }
        assert!(Hosts::parse(&[" ".into()]).unwrap().is_empty());
    }
}
