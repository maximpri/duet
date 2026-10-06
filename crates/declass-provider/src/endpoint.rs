// SPDX-License-Identifier: GPL-3.0-or-later
//! Endpoint trust: the local role may only talk to this machine or an owner-allowlisted host.

use crate::error::{ErrorKind, ProviderError};

/// An immutable model endpoint admitted for an explicit role. Model-listing
/// and diagnostic APIs require this token, so adding a caller cannot omit
/// the local endpoint trust check before sending its credentials.
#[derive(Debug, Clone)]
pub struct ApprovedEndpoint {
    url: reqwest::Url,
}

impl ApprovedEndpoint {
    pub fn new(base_url: &str, role: &crate::Role) -> Result<Self, ProviderError> {
        let url = reqwest::Url::parse(base_url).map_err(|_| {
            ProviderError::new(
                ErrorKind::Forbidden,
                "model endpoint must be a valid HTTP(S) URL",
            )
        })?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ProviderError::new(
                ErrorKind::Forbidden,
                "model endpoint must be HTTP(S), without URL credentials, query or fragment",
            ));
        }
        if let crate::Role::Local {
            allowlist,
            allow_plaintext,
        } = role
        {
            check_local_endpoint(url.as_str(), allowlist, *allow_plaintext)?;
        }
        Ok(Self { url })
    }

    pub fn as_str(&self) -> &str {
        self.url.as_str().trim_end_matches('/')
    }

    /// A wire-dialect adapter can change the API path, never the recipient.
    pub(crate) fn permits(&self, target: &str) -> bool {
        reqwest::Url::parse(target).is_ok_and(|url| {
            url.origin() == self.url.origin()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
        })
    }
}

/// Host and port of an `http(s)://host[:port]/...` URL.
pub fn host_port(base_url: &str) -> Option<(String, u16)> {
    parsed_endpoint(base_url).map(|(host, port, _)| (host, port))
}

/// Use the HTTP client's parser for trust decisions, including URL
/// normalization (backslashes, userinfo, IPv4 spelling and whitespace).
fn parsed_endpoint(base_url: &str) -> Option<(String, u16, bool)> {
    let url = reqwest::Url::parse(base_url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url
        .host_str()?
        .trim_start_matches('[')
        .trim_end_matches(']');
    Some((
        host.to_owned(),
        url.port_or_known_default()?,
        url.scheme() == "https",
    ))
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
    let (host, port, tls) = parsed_endpoint(base_url).ok_or_else(|| {
        ProviderError::new(
            ErrorKind::Forbidden,
            "local endpoint must be a valid HTTP(S) URL",
        )
    })?;
    if is_loopback_host(&host) {
        return Ok(Trust::Loopback);
    }
    let authority = format!("{host}:{port}");
    if !allowlist.iter().any(|a| a.eq_ignore_ascii_case(&authority)) {
        return Err(ProviderError::new(
            ErrorKind::Forbidden,
            format!(
                "local model endpoint {host}:{port} is neither loopback nor allowlisted by the owner"
            ),
        ));
    }
    if tls {
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
so the sensitive content Declass sends it (secrets, personal data, data files) would cross the network \
unencrypted, readable and alterable by anyone on the path.\n\
Fix it in one of two ways:\n\
  1. Encrypt the path: serve the model over TLS (local.base_url = \"https://...\"), or reach it \
through an SSH tunnel on loopback (ssh -N -L 8080:127.0.0.1:{port} user@{host}, then \
local.base_url = \"http://127.0.0.1:8080/v1\").\n\
  2. Accept the risk on a network you control: set local.allow_plaintext = true in the owner \
config (declass config set local.allow_plaintext true --confirm). A project config cannot set it."
    )
}

/// Whether sensitive content to this endpoint would cross the network unencrypted.
pub fn is_plaintext_remote(base_url: &str) -> bool {
    parsed_endpoint(base_url).is_some_and(|(host, _, tls)| !tls && !is_loopback_host(&host))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_is_role_bound_and_rejects_credentials_without_echoing_them() {
        let local = crate::Role::Local {
            allowlist: Vec::new(),
            allow_plaintext: false,
        };
        assert!(ApprovedEndpoint::new("https://untrusted.example/v1", &local).is_err());
        assert!(
            ApprovedEndpoint::new("https://provider.example/v1", &crate::Role::Frontier).is_ok()
        );
        for url in [
            "http://user:fixture-password@127.0.0.1/v1",
            "http://127.0.0.1/v1?key=fixture-password",
            "http://127.0.0.1/v1#fixture-password",
        ] {
            let error = ApprovedEndpoint::new(url, &local).unwrap_err();
            assert!(!error.message.contains("fixture-password"));
        }
    }

    #[test]
    fn an_approved_endpoint_allows_native_paths_but_never_another_recipient() {
        let endpoint =
            ApprovedEndpoint::new("https://provider.example/v1", &crate::Role::Frontier).unwrap();
        assert!(endpoint.permits("https://provider.example/api/show"));
        for url in [
            "https://elsewhere.example/v1/chat/completions",
            "http://provider.example/v1/chat/completions",
            "https://provider.example:8443/v1/chat/completions",
            "https://user:fixture@provider.example/v1/chat/completions",
        ] {
            assert!(!endpoint.permits(url));
        }
    }

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

    #[test]
    fn trust_uses_the_same_url_normalization_as_the_http_client() {
        // A backslash ends the authority in an HTTP URL. Splitting at '@'
        // instead would incorrectly trust this remote destination as local.
        let deceptive = "http://remote.example\\@localhost/v1";
        assert_eq!(host_port(deceptive), Some(("remote.example".into(), 80)));
        assert!(check_local_endpoint(deceptive, &[], false).is_err());

        let lan = vec!["10.0.0.2:80".into()];
        for url in [
            " http://10.0.0.2/v1",
            "\nHTTP://10.0.0.2/v1",
            "ht\ttp://10.0.0.2/v1",
        ] {
            assert!(is_plaintext_remote(url), "{url:?}");
            assert!(check_local_endpoint(url, &lan, false).is_err(), "{url:?}");
            assert_eq!(
                check_local_endpoint(url, &lan, true).unwrap(),
                Trust::AllowlistedPlaintext
            );
        }
        for url in [
            "ftp://localhost/v1",
            "file://localhost/v1",
            "http://[::1]suffix/v1",
        ] {
            assert!(host_port(url).is_none(), "{url}");
            assert!(check_local_endpoint(url, &[], true).is_err(), "{url}");
        }
        for url in [
            "http://127.1/v1",
            "http://2130706433/v1",
            "http://0x7f000001/v1",
        ] {
            assert_eq!(host_port(url), Some(("127.0.0.1".into(), 80)));
            assert_eq!(
                check_local_endpoint(url, &[], false).unwrap(),
                Trust::Loopback
            );
        }
    }
}
