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

/// Why the local role may use an endpoint (recorded in the run's audit log).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// This machine.
    Loopback,
    /// An owner-allowlisted host reached over TLS.
    AllowlistedTls,
    /// An owner-allowlisted host over plain HTTP, accepted because the owner set
    /// `local.allow_plaintext`.
    AllowlistedPlaintext,
}

impl Trust {
    pub fn as_str(self) -> &'static str {
        match self {
            Trust::Loopback => "loopback",
            Trust::AllowlistedTls => "allowlisted-tls",
            Trust::AllowlistedPlaintext => "allowlisted-plaintext-opted-in",
        }
    }
}

/// Admits `base_url` for the local role only if it is loopback or exactly an
/// owner-allowlisted `host:port`, and, for a non-loopback host, only over TLS
/// unless the owner opted into plain HTTP (`allow_plaintext`).
pub fn check_local_endpoint(
    base_url: &str,
    allowlist: &[String],
    allow_plaintext: bool,
) -> Result<Trust, ProviderError> {
    let (host, port) = host_port(base_url).ok_or_else(|| {
        ProviderError::new(
            ErrorKind::Forbidden,
            format!("cannot parse local endpoint {base_url}"),
        )
    })?;
    if is_loopback_host(&host) {
        return Ok(Trust::Loopback);
    }
    if !allowlist
        .iter()
        .any(|a| a.eq_ignore_ascii_case(&format!("{host}:{port}")))
    {
        return Err(ProviderError::new(
            ErrorKind::Forbidden,
            format!(
                "local model endpoint {host}:{port} is neither loopback nor allowlisted by the owner"
            ),
        ));
    }
    if !is_plaintext_remote(base_url) {
        return Ok(Trust::AllowlistedTls);
    }
    if allow_plaintext {
        return Ok(Trust::AllowlistedPlaintext);
    }
    Err(ProviderError::new(
        ErrorKind::Forbidden,
        plaintext_refusal(&host, port),
    ))
}

/// The refusal for a remote local model over plain HTTP: the risk and both fixes.
pub fn plaintext_refusal(host: &str, port: u16) -> String {
    format!(
        "refusing the local model endpoint http://{host}:{port}: it is a remote host over plain HTTP, \
so the sensitive content Duet sends it (secrets, personal data, data files) would cross the network \
unencrypted, readable and alterable by anyone on the path.\n\
Fix it in one of two ways:\n\
  1. Encrypt the path: serve the model over TLS (local.base_url = \"https://...\"), or reach it \
through an SSH tunnel on loopback (ssh -N -L 8080:127.0.0.1:{port} user@{host}, then \
local.base_url = \"http://127.0.0.1:8080/v1\").\n\
  2. Accept the risk on a network you control: set local.allow_plaintext = true in the owner \
config (duet config set local.allow_plaintext true --confirm). A project config cannot set it."
    )
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
        let lan = || vec!["192.168.50.132:8080".to_string()];
        assert_eq!(
            check_local_endpoint("http://127.0.0.1:11434/v1", &[], false).unwrap(),
            Trust::Loopback
        );
        assert!(check_local_endpoint("http://localhost:1234/v1", &[], false).is_ok());
        assert!(check_local_endpoint("http://192.168.50.132:8080/v1", &[], true).is_err());
        assert_eq!(
            check_local_endpoint("http://192.168.50.132:8080/v1", &lan(), true).unwrap(),
            Trust::AllowlistedPlaintext
        );
        assert!(check_local_endpoint("http://192.168.50.132:9999/v1", &lan(), true).is_err());
        assert!(check_local_endpoint("https://api.z.ai/api/coding/paas/v4", &[], true).is_err());
    }

    #[test]
    fn plaintext_to_a_remote_host_needs_the_owner_opt_in() {
        let lan = vec!["192.168.50.132:8080".to_string()];
        let e = check_local_endpoint("http://192.168.50.132:8080/v1", &lan, false).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Forbidden);
        for needed in [
            "plain HTTP",
            "TLS",
            "SSH tunnel",
            "local.allow_plaintext = true",
        ] {
            assert!(e.message.contains(needed), "{needed}: {}", e.message);
        }
        // TLS to an allowlisted host and loopback over HTTP need no opt-in.
        let tls = vec!["10.0.0.2:443".to_string()];
        assert_eq!(
            check_local_endpoint("https://10.0.0.2/v1", &tls, false).unwrap(),
            Trust::AllowlistedTls
        );
        assert!(check_local_endpoint("http://[::1]:8080/v1", &[], false).is_ok());
    }

    #[test]
    fn flags_plaintext_remote() {
        assert!(is_plaintext_remote("http://192.168.50.132:8080/v1"));
        assert!(!is_plaintext_remote("http://127.0.0.1:8080/v1"));
        assert!(!is_plaintext_remote("https://10.0.0.2/v1"));
    }
}
