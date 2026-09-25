// SPDX-License-Identifier: GPL-3.0-or-later
//! The guarded fetch and the search backends against a local mock HTTP server.
//! Names are resolved by a scripted resolver, so "a public name that resolves
//! to a private address" is tested without DNS. Nothing here leaves the host.

use duet_web::guard::Allowlist;
use duet_web::search::Backend;
use duet_web::{Resolve, Web, WebConfig, WebError};
use futures_util::future::BoxFuture;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A canned response: status, extra headers, body.
type Reply = (u16, Vec<(String, String)>, Vec<u8>);

/// One request the server received: its path and headers (lower-cased names).
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: HashMap<String, String>,
}

struct Server {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Server {
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
    fn base(&self, host: &str) -> String {
        format!("http://{host}:{}", self.addr.port())
    }
}

/// Serves `routes` (path without query → reply); `/slow` never answers.
async fn serve(routes: HashMap<&'static str, Reply>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = seen.clone();
    let routes = Arc::new(routes);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let log = log.clone();
            let routes = routes.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).into_owned();
                let mut lines = head.lines();
                let target = lines
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_owned();
                let headers = lines
                    .filter_map(|l| l.split_once(':'))
                    .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
                    .collect();
                let path = target.split('?').next().unwrap_or("/").to_owned();
                log.lock().unwrap().push(Seen {
                    path: target.clone(),
                    headers,
                });
                if path == "/slow" {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    return;
                }
                let (status, extra, body) = routes.get(path.as_str()).cloned().unwrap_or((
                    404,
                    vec![],
                    b"not found".to_vec(),
                ));
                let mut resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for (k, v) in extra {
                    resp.push_str(&format!("{k}: {v}\r\n"));
                }
                resp.push_str("\r\n");
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.write_all(&body).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    Server { addr, seen }
}

/// Resolves the given names to fixed addresses; anything else fails.
struct Names(HashMap<String, Vec<SocketAddr>>);

impl Resolve for Names {
    fn resolve(
        &self,
        host: String,
        _port: u16,
    ) -> BoxFuture<'static, std::io::Result<Vec<SocketAddr>>> {
        let r = self
            .0
            .get(&host)
            .cloned()
            .ok_or_else(|| std::io::Error::other("no such host"));
        Box::pin(async move { r })
    }
}

fn reply(status: u16, content_type: &str, body: &str) -> Reply {
    (
        status,
        vec![("Content-Type".into(), content_type.into())],
        body.as_bytes().to_vec(),
    )
}

fn redirect(to: &str) -> Reply {
    (302, vec![("Location".into(), to.into())], Vec::new())
}

fn web(server: &Server, allow: &[&str], max_bytes: usize, search: Option<Backend>) -> Web {
    let loopback = SocketAddr::new(server.addr.ip(), server.addr.port());
    let names = Names(
        ["intranet.test", "evil.test", "public.test"]
            .into_iter()
            .map(|n| (n.to_owned(), vec![loopback]))
            .collect(),
    );
    Web::new(WebConfig {
        max_bytes,
        timeout: Duration::from_secs(2),
        allowlist: Allowlist::parse(&allow.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
            .unwrap(),
        search,
    })
    .with_resolver(Arc::new(names))
}

const PAGE: &str = "<html><head><title>Docs</title><script>steal()</script></head>\
<body><h2>Install</h2><p>Run <a href=\"/guide\">the guide</a>.</p></body></html>";

#[tokio::test]
async fn loopback_and_names_resolving_to_private_addresses_are_refused_before_connecting() {
    let s = serve(HashMap::from([("/", reply(200, "text/plain", "hi"))])).await;
    let w = web(&s, &[], 10_000, None);
    for url in [
        s.base("127.0.0.1"),
        s.base("evil.test"),
        format!("http://[::1]:{}/", s.addr.port()),
        format!("http://[::ffff:127.0.0.1]:{}/", s.addr.port()),
        "http://169.254.169.254/latest/meta-data/".into(),
        "http://metadata.google.internal/".into(),
        "http://10.0.0.8/".into(),
        "http://100.64.1.1/".into(),
        "http://[fd00::1]/".into(),
    ] {
        let e = w.fetch(&url).await.unwrap_err();
        assert!(matches!(e, WebError::Refused(_)), "{url}: {e}");
    }
    assert!(s.seen().is_empty(), "{:?}", s.seen());
    // Even an allowlisted link-local network never opens the metadata address.
    let w = web(&s, &["169.254.0.0/16"], 10_000, None);
    assert!(matches!(
        w.fetch("http://169.254.169.254/").await,
        Err(WebError::Refused(_))
    ));
}

#[tokio::test]
async fn an_allowlisted_host_is_fetched_at_the_checked_address_and_html_becomes_text() {
    let s = serve(HashMap::from([(
        "/docs",
        reply(200, "text/html; charset=utf-8", PAGE),
    )]))
    .await;
    let w = web(&s, &["intranet.test"], 10_000, None);
    let page = w
        .fetch(&format!("{}/docs", s.base("intranet.test")))
        .await
        .unwrap();
    assert_eq!(page.status, 200);
    assert!(page.text.starts_with("Title: Docs"), "{}", page.text);
    assert!(page.text.contains("## Install"), "{}", page.text);
    let guide = format!("[the guide]({}/guide)", s.base("intranet.test"));
    assert!(page.text.contains(&guide), "{}", page.text);
    assert!(!page.text.contains("steal"), "{}", page.text);
    let seen = s.seen();
    assert_eq!(seen.len(), 1);
    // Connected to the pinned address, under the requested name.
    assert_eq!(
        seen[0].headers["host"],
        format!("intranet.test:{}", s.addr.port())
    );
    assert!(seen[0].headers["user-agent"].starts_with("duet/"));
    // The allowlisted name does not open other names at the same address.
    assert!(matches!(
        w.fetch(&format!("{}/docs", s.base("evil.test"))).await,
        Err(WebError::Refused(_))
    ));
}

#[tokio::test]
async fn every_redirect_is_checked_and_their_number_is_limited() {
    let s = serve(HashMap::from([
        ("/start", redirect("/next")),
        ("/next", reply(200, "text/plain", "arrived")),
        ("/loop", redirect("/loop")),
        ("/to-file", redirect("file:///etc/passwd")),
    ]))
    .await;
    let w = web(&s, &["intranet.test"], 10_000, None);
    let page = w
        .fetch(&format!("{}/start", s.base("intranet.test")))
        .await
        .unwrap();
    assert_eq!(page.text, "arrived");
    assert!(page.url.ends_with("/next"));

    let e = w
        .fetch(&format!("{}/loop", s.base("intranet.test")))
        .await
        .unwrap_err();
    assert_eq!(e, WebError::TooManyRedirects);
    let loops = s.seen().iter().filter(|r| r.path == "/loop").count();
    assert_eq!(loops, duet_web::MAX_REDIRECTS + 1);

    let e = w
        .fetch(&format!("{}/to-file", s.base("intranet.test")))
        .await
        .unwrap_err();
    assert!(matches!(e, WebError::Refused(_)), "{e}");
}

#[tokio::test]
async fn a_redirect_to_loopback_is_refused_before_it_is_followed() {
    // The first server redirects to the second, by loopback address.
    let target = serve(HashMap::from([(
        "/secret",
        reply(200, "text/plain", "internal"),
    )]))
    .await;
    let s = serve(HashMap::from([(
        "/go",
        redirect(&format!("http://127.0.0.1:{}/secret", target.addr.port())),
    )]))
    .await;
    let w = web(&s, &["public.test"], 10_000, None);
    let e = w
        .fetch(&format!("{}/go", s.base("public.test")))
        .await
        .unwrap_err();
    assert!(
        matches!(e, WebError::Refused(ref m) if m.contains("loopback")),
        "{e}"
    );
    assert_eq!(s.seen().len(), 1);
    assert!(
        target.seen().is_empty(),
        "the redirect target was contacted"
    );
}

#[tokio::test]
async fn bodies_are_capped_binary_is_refused_and_slow_servers_time_out() {
    let big = "x".repeat(100_000);
    let s = serve(HashMap::from([
        ("/big", reply(200, "text/plain", &big)),
        (
            "/logo",
            (
                200,
                vec![("Content-Type".into(), "image/png".into())],
                vec![0x89, b'P', b'N', b'G', 0, 0],
            ),
        ),
        ("/untyped", (200, vec![], vec![1, 2, 0, 3])),
        (
            "/untyped-html",
            (200, vec![], b"<!DOCTYPE html><p>hello</p>".to_vec()),
        ),
        ("/api", reply(200, "application/json", "{\"a\": [1, 2]}")),
    ]))
    .await;
    let w = web(&s, &["intranet.test"], 1000, None);
    let base = s.base("intranet.test");
    let page = w.fetch(&format!("{base}/big")).await.unwrap();
    assert!(page.truncated);
    assert_eq!(page.bytes, 1000);
    assert_eq!(page.text.len(), 1000);
    assert!(matches!(
        w.fetch(&format!("{base}/logo")).await,
        Err(WebError::Binary(_))
    ));
    assert!(matches!(
        w.fetch(&format!("{base}/untyped")).await,
        Err(WebError::Binary(_))
    ));
    assert_eq!(
        w.fetch(&format!("{base}/untyped-html")).await.unwrap().text,
        "hello\n"
    );
    assert_eq!(
        w.fetch(&format!("{base}/api")).await.unwrap().text,
        "{\"a\": [1, 2]}"
    );
    let started = std::time::Instant::now();
    assert!(matches!(
        w.fetch(&format!("{base}/slow")).await,
        Err(WebError::Timeout(_))
    ));
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn searxng_and_brave_replies_become_results() {
    let searx = r#"{"query":"rust","results":[
        {"title":"Rust","url":"https://www.rust-lang.org/","content":"A language empowering everyone"},
        {"title":"Book","url":"https://doc.rust-lang.org/book/","content":"The book"}]}"#;
    let brave = r#"{"type":"search","web":{"results":[
        {"title":"<strong>Rust</strong> docs","url":"https://doc.rust-lang.org/","description":"Official &amp; free"}]}}"#;
    let s = serve(HashMap::from([
        ("/searx/search", reply(200, "application/json", searx)),
        ("/brave", reply(200, "application/json", brave)),
        ("/broken/search", reply(500, "text/plain", "oops")),
    ]))
    .await;
    let base =
        |p: &str| url::Url::parse(&format!("http://127.0.0.1:{}{p}", s.addr.port())).unwrap();

    let w = web(
        &s,
        &[],
        10_000,
        Some(Backend::Searxng {
            base: base("/searx"),
        }),
    );
    let r = w.search("rust lang", 1).await.unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].url, "https://www.rust-lang.org/");
    assert_eq!(r[0].snippet, "A language empowering everyone");
    let q = s.seen().last().unwrap().path.clone();
    assert!(
        q.contains("q=rust+lang") && q.contains("format=json"),
        "{q}"
    );

    let key = "brave-test-key-0192";
    let w = web(
        &s,
        &[],
        10_000,
        Some(Backend::Brave {
            endpoint: base("/brave"),
            key: key.into(),
        }),
    );
    let r = w.search("rust", 3).await.unwrap();
    assert_eq!(r[0].title, "Rust docs");
    assert_eq!(r[0].snippet, "Official & free");
    let seen = s.seen().last().unwrap().clone();
    assert_eq!(seen.headers["x-subscription-token"], key);
    assert!(seen.path.contains("count=3"), "{}", seen.path);

    let w = web(
        &s,
        &[],
        10_000,
        Some(Backend::Searxng {
            base: base("/broken"),
        }),
    );
    assert!(matches!(w.search("x", 5).await, Err(WebError::Search(_))));
    let w = web(&s, &[], 10_000, None);
    assert!(matches!(w.search("x", 5).await, Err(WebError::Search(_))));
}
