// SPDX-License-Identifier: GPL-3.0-or-later
//! Endpoint trust: the local role may only talk to this machine or an owner-allowlisted host.

use crate::error::{ErrorKind, ProviderError};

/// Host and port of an `http(s)://host[:port]/...` URL.
pub fn host_port(base_url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = base_url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let default = if scheme.eq_ignore_ascii_case("https") {
        443
    } else {
        80
    };
    if let Some(v6) = authority.strip_prefix('[') {
        let (host, rest) = v6.split_once(']')?;
        let port = rest
            .strip_prefix(':')
            .map_or(Some(default), |p| p.parse().ok())?;
        return Some((host.to_ascii_lowercase(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_ascii_lowercase(), port.parse().ok()?)),
        None => Some((authority.to_ascii_lowercase(), default)),
    }
}

pub fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Admits `base_url` for the local role only if it is loopback or exactly an
/// owner-allowlisted `host:port`.
pub fn check_local_endpoint(base_url: &str, allowlist: &[String]) -> Result<(), ProviderError> {
    let (host, port) = host_port(base_url).ok_or_else(|| {
        ProviderError::new(
            ErrorKind::Forbidden,
            format!("cannot parse local endpoint {base_url}"),
        )
    })?;
    if is_loopback_host(&host)
        || allowlist
            .iter()
            .any(|a| a.eq_ignore_ascii_case(&format!("{host}:{port}")))
    {
        Ok(())
    } else {
        Err(ProviderError::new(
            ErrorKind::Forbidden,
            format!(
                "local model endpoint {host}:{port} is neither loopback nor allowlisted by the owner"
            ),
        ))
    }
}

/// Whether sensitive content to this endpoint would cross the network unencrypted.
pub fn is_plaintext_remote(base_url: &str) -> bool {
    base_url.to_ascii_lowercase().starts_with("http://")
        && host_port(base_url).is_some_and(|(h, _)| !is_loopback_host(&h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hosts() {
        assert_eq!(
            host_port("http://192.168.50.132:8080/v1"),
            Some(("192.168.50.132".into(), 8080))
        );
        assert_eq!(
            host_port("https://api.z.ai/api/coding/paas/v4"),
            Some(("api.z.ai".into(), 443))
        );
        assert_eq!(
            host_port("http://[::1]:1234/v1"),
            Some(("::1".into(), 1234))
        );
        assert_eq!(
            host_port("http://user:pw@LOCALHOST:11434"),
            Some(("localhost".into(), 11434))
        );
    }

    #[test]
    fn local_role_admits_loopback_and_allowlist_only() {
        assert!(check_local_endpoint("http://127.0.0.1:11434/v1", &[]).is_ok());
        assert!(check_local_endpoint("http://localhost:1234/v1", &[]).is_ok());
        assert!(check_local_endpoint("http://192.168.50.132:8080/v1", &[]).is_err());
        assert!(
            check_local_endpoint(
                "http://192.168.50.132:8080/v1",
                &["192.168.50.132:8080".into()]
            )
            .is_ok()
        );
        assert!(
            check_local_endpoint(
                "http://192.168.50.132:9999/v1",
                &["192.168.50.132:8080".into()]
            )
            .is_err()
        );
        assert!(check_local_endpoint("https://api.z.ai/api/coding/paas/v4", &[]).is_err());
    }

    #[test]
    fn flags_plaintext_remote() {
        assert!(is_plaintext_remote("http://192.168.50.132:8080/v1"));
        assert!(!is_plaintext_remote("http://127.0.0.1:8080/v1"));
        assert!(!is_plaintext_remote("https://10.0.0.2/v1"));
    }
}
