// SPDX-License-Identifier: GPL-3.0-or-later
//! The head of a request a client sends to the proxy.
//!
//! Two forms are served: `CONNECT host:port` (a tunnel, for HTTPS) and a
//! plain-HTTP `GET` or `HEAD` in absolute form (`GET http://host/path`),
//! which is sent on with the host header set from the target and the
//! connection closed after the response. Anything else is refused.

/// Longest request head accepted.
pub const MAX_HEAD: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Connect {
        host: String,
        port: u16,
    },
    /// A plain-HTTP request, rewritten for the origin server.
    Forward {
        host: String,
        port: u16,
        /// The head to send: origin-form request line, the client's end-to-end
        /// headers, `Host` from the target, `Connection: close`.
        head: Vec<u8>,
    },
}

impl Request {
    pub fn target(&self) -> (&str, u16) {
        match self {
            Request::Connect { host, port } | Request::Forward { host, port, .. } => (host, *port),
        }
    }
}

/// Why a head was refused: the status to answer with and a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub status: u16,
    pub reason: String,
    /// The target's host and port when they could be read (for the audit).
    pub target: Option<(String, u16)>,
}

fn refuse(status: u16, reason: impl Into<String>) -> Refusal {
    Refusal {
        status,
        reason: reason.into(),
        target: None,
    }
}

/// Hop-by-hop headers and the proxy's own, never sent on.
const DROPPED: &[&str] = &[
    "host",
    "connection",
    "proxy-connection",
    "keep-alive",
    "proxy-authorization",
    "proxy-authenticate",
    "te",
    "trailer",
    "upgrade",
];

/// The end of the head in `buf` (the index after `\r\n\r\n`), if complete.
pub fn head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// Parses a complete head (up to and including the blank line).
pub fn parse(head: &[u8]) -> Result<Request, Refusal> {
    let text =
        std::str::from_utf8(head).map_err(|_| refuse(400, "a request head that is not text"))?;
    let mut lines = text.split("\r\n");
    let line = lines.next().unwrap_or_default();
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(refuse(400, "a malformed request line"));
    };
    if !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        return Err(refuse(400, "not HTTP/1.x"));
    }
    let headers: Vec<(&str, &str)> = lines
        .take_while(|l| !l.is_empty())
        .map(|l| {
            l.split_once(':')
                .map(|(k, v)| (k.trim(), v.trim()))
                .ok_or_else(|| refuse(400, "a malformed header"))
        })
        .collect::<Result<_, _>>()?;
    match method {
        "CONNECT" => {
            let (host, port) =
                authority(target).ok_or_else(|| refuse(400, "CONNECT needs host:port"))?;
            Ok(Request::Connect { host, port })
        }
        "GET" | "HEAD" => forward(method, target, version, &headers),
        _ => {
            // Only reads cross in plain HTTP; uploads need a tunnel (and the
            // tunnel's host passes the same check).
            let mut r = refuse(405, format!("{method} is not forwarded in plain HTTP"));
            r.target = url::Url::parse(target)
                .ok()
                .and_then(|u| Some((u.host_str()?.to_owned(), u.port_or_known_default()?)));
            Err(r)
        }
    }
}

/// `host:port` of a CONNECT target (the port is required).
fn authority(target: &str) -> Option<(String, u16)> {
    let (host, port) = target.rsplit_once(':')?;
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    (!host.is_empty()).then(|| (host.to_owned(), port))
}

fn forward(
    method: &str,
    target: &str,
    version: &str,
    headers: &[(&str, &str)],
) -> Result<Request, Refusal> {
    let url = url::Url::parse(target).map_err(|_| {
        refuse(
            400,
            "a plain-HTTP request must name its URL (absolute form)",
        )
    })?;
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return Err(refuse(400, "the URL has no host"));
    };
    let target_of = || Some((host.to_owned(), port));
    if url.scheme() != "http" {
        let mut r = refuse(
            400,
            format!("{}: URLs go through a CONNECT tunnel", url.scheme()),
        );
        r.target = target_of();
        return Err(r);
    }
    if !url.username().is_empty() || url.password().is_some() {
        let mut r = refuse(400, "URLs with credentials are not forwarded");
        r.target = target_of();
        return Err(r);
    }
    let has_body = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("transfer-encoding")
            || (k.eq_ignore_ascii_case("content-length") && v.trim() != "0")
    });
    if has_body {
        let mut r = refuse(400, "a plain-HTTP request with a body is not forwarded");
        r.target = target_of();
        return Err(r);
    }
    let mut path = url.path().to_owned();
    if let Some(q) = url.query() {
        path.push('?');
        path.push_str(q);
    }
    let host_header = match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_owned(),
    };
    let mut head = format!("{method} {path} {version}\r\nHost: {host_header}\r\n");
    for (k, v) in headers {
        if !DROPPED.iter().any(|d| k.eq_ignore_ascii_case(d)) {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    head.push_str("Connection: close\r\n\r\n");
    Ok(Request::Forward {
        host: host.to_owned(),
        port,
        head: head.into_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_names_host_and_port() {
        assert_eq!(
            parse(
                b"CONNECT registry.npmjs.org:443 HTTP/1.1\r\nHost: registry.npmjs.org:443\r\n\r\n"
            ),
            Ok(Request::Connect {
                host: "registry.npmjs.org".into(),
                port: 443
            })
        );
        assert_eq!(
            parse(b"CONNECT example.com HTTP/1.1\r\n\r\n")
                .unwrap_err()
                .status,
            400
        );
    }

    #[test]
    fn plain_requests_are_rewritten_with_their_own_host() {
        let Ok(Request::Forward { host, port, head }) = parse(
            b"GET http://files.example.test:8080/a/b?c=1 HTTP/1.1\r\nHost: other.test\r\n\
              Proxy-Connection: keep-alive\r\nProxy-Authorization: Basic x\r\nAccept: */*\r\n\r\n",
        ) else {
            panic!()
        };
        assert_eq!((host.as_str(), port), ("files.example.test", 8080));
        let head = String::from_utf8(head).unwrap();
        assert_eq!(
            head,
            "GET /a/b?c=1 HTTP/1.1\r\nHost: files.example.test:8080\r\nAccept: */*\r\nConnection: close\r\n\r\n"
        );
    }

    #[test]
    fn uploads_bodies_and_other_forms_are_refused() {
        for (head, status) in [
            (
                &b"POST http://a.test/ HTTP/1.1\r\nContent-Length: 3\r\n\r\n"[..],
                405,
            ),
            (
                b"GET http://a.test/ HTTP/1.1\r\nContent-Length: 3\r\n\r\n",
                400,
            ),
            (
                b"GET http://a.test/ HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
                400,
            ),
            (b"GET /origin-form HTTP/1.1\r\nHost: a.test\r\n\r\n", 400),
            (b"GET https://a.test/ HTTP/1.1\r\n\r\n", 400),
            (b"GET http://u:p@a.test/ HTTP/1.1\r\n\r\n", 400),
            (b"GET http://a.test/ HTTP/2\r\n\r\n", 400),
            (b"GET  http://a.test/ HTTP/1.1\r\n\r\n", 400),
            (b"GET http://a.test/ HTTP/1.1\r\nno colon\r\n\r\n", 400),
        ] {
            let r = parse(head).unwrap_err();
            assert_eq!(r.status, status, "{}", String::from_utf8_lossy(head));
        }
        // The target is still known for the audit.
        let r = parse(b"POST http://a.test/x HTTP/1.1\r\n\r\n").unwrap_err();
        assert_eq!(r.target, Some(("a.test".into(), 80)));
    }

    #[test]
    fn the_head_ends_at_the_blank_line() {
        assert_eq!(head_end(b"GET / HTTP/1.1\r\n\r\nbody"), Some(18));
        assert_eq!(head_end(b"GET / HTTP/1.1\r\n"), None);
    }
}
