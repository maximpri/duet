// SPDX-License-Identifier: GPL-3.0-or-later
//! The guarded fetch and the search backends against a local mock HTTP server.
//! Names are resolved by a scripted resolver, so "a public name that resolves
//! to a private address" is tested without DNS. Nothing here leaves the host.

use declass_boundary::third_party::Guard;
use declass_boundary::view::{PassThrough, Presenter};
use declass_web::guard::Allowlist;
use declass_web::search::Backend;
use declass_web::{Resolve, Web, WebConfig, WebError};
use futures_util::future::BoxFuture;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A canned response: status, extra headers, body.
type Reply = (u16, Vec<(String, String)>, Vec<u8>);

/// One request the server received: its method, path, headers (lower-cased
/// names) and body.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
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
                let end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                let head = String::from_utf8_lossy(&buf[..end]).into_owned();
                let mut lines = head.lines();
                let first: Vec<String> = lines
                    .next()
                    .unwrap_or_default()
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect();
                let method = first.first().cloned().unwrap_or_default();
                let target = first.get(1).cloned().unwrap_or_else(|| "/".into());
                let headers: HashMap<String, String> = lines
                    .filter_map(|l| l.split_once(':'))
                    .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
                    .collect();
                let length: usize = headers
                    .get("content-length")
                    .and_then(|l| l.parse().ok())
                    .unwrap_or(0);
                let mut body = buf[end..].to_vec();
                while body.len() < length {
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => body.extend_from_slice(&tmp[..n]),
                    }
                }
                let path = target.split('?').next().unwrap_or("/").to_owned();
                log.lock().unwrap().push(Seen {
                    method,
                    path: target.clone(),
                    headers,
                    body: String::from_utf8_lossy(&body).into_owned(),
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
        let e = w.fetch(&open(), &url).await.unwrap_err();
        assert!(matches!(e, WebError::Refused(_)), "{url}: {e}");
    }
    assert!(s.seen().is_empty(), "{:?}", s.seen());
    // Even an allowlisted link-local network never opens the metadata address.
    let w = web(&s, &["169.254.0.0/16"], 10_000, None);
    assert!(matches!(
        w.fetch(&open(), "http://169.254.169.254/").await,
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
        .fetch(&open(), &format!("{}/docs", s.base("intranet.test")))
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
    assert!(seen[0].headers["user-agent"].starts_with("declass/"));
    // The allowlisted name does not open other names at the same address.
    assert!(matches!(
        w.fetch(&open(), &format!("{}/docs", s.base("evil.test")))
            .await,
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
        .fetch(&open(), &format!("{}/start", s.base("intranet.test")))
        .await
        .unwrap();
    assert_eq!(page.text, "arrived");
    assert!(page.url.ends_with("/next"));

    let e = w
        .fetch(&open(), &format!("{}/loop", s.base("intranet.test")))
        .await
        .unwrap_err();
    assert_eq!(e, WebError::TooManyRedirects);
    let loops = s.seen().iter().filter(|r| r.path == "/loop").count();
    assert_eq!(loops, declass_web::MAX_REDIRECTS + 1);

    let e = w
        .fetch(&open(), &format!("{}/to-file", s.base("intranet.test")))
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
        .fetch(&open(), &format!("{}/go", s.base("public.test")))
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
    let page = w.fetch(&open(), &format!("{base}/big")).await.unwrap();
    assert!(page.truncated);
    assert_eq!(page.bytes, 1000);
    assert_eq!(page.text.len(), 1000);
    assert!(matches!(
        w.fetch(&open(), &format!("{base}/logo")).await,
        Err(WebError::Binary(_))
    ));
    assert!(matches!(
        w.fetch(&open(), &format!("{base}/untyped")).await,
        Err(WebError::Binary(_))
    ));
    assert_eq!(
        w.fetch(&open(), &format!("{base}/untyped-html"))
            .await
            .unwrap()
            .text,
        "hello\n"
    );
    assert_eq!(
        w.fetch(&open(), &format!("{base}/api")).await.unwrap().text,
        "{\"a\": [1, 2]}"
    );
    let started = std::time::Instant::now();
    assert!(matches!(
        w.fetch(&open(), &format!("{base}/slow")).await,
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
    let r = w.search(&open(), "rust lang", 1).await.unwrap();
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
    let r = w.search(&open(), "rust", 3).await.unwrap();
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
    assert!(matches!(
        w.search(&open(), "x", 5).await,
        Err(WebError::Search(_))
    ));
    let w = web(&s, &[], 10_000, None);
    assert!(matches!(
        w.search(&open(), "x", 5).await,
        Err(WebError::Search(_))
    ));
}

#[tokio::test]
async fn zai_is_a_post_with_the_key_in_a_header_and_wikipedia_a_paced_get() {
    let zai = r#"{"created":1790383976,"id":"t1","search_result":[
        {"content":"The Adventures of Captain Comic is a platform game written by Michael Denio",
         "icon":"","link":"https://en.wikipedia.org","media":"","publish_date":"","refer":"ref_1",
         "title":"The Adventures of Captain Comic"}]}"#;
    let wiki = r#"{"batchcomplete":true,"query":{"search":[
        {"ns":0,"title":"The Adventures of Captain Comic","pageid":1558412,
         "snippet":"a platform <span class=\"searchmatch\">game</span> released as shareware in 1988"}]}}"#;
    let s = serve(HashMap::from([
        (
            "/api/coding/paas/v4/web_search",
            reply(200, "application/json", zai),
        ),
        ("/w/api.php", reply(200, "application/json", wiki)),
        ("/quota/web_search", reply(429, "application/json", "{}")),
        ("/denied/web_search", reply(401, "text/plain", "key abc")),
    ]))
    .await;
    let base =
        |p: &str| url::Url::parse(&format!("http://127.0.0.1:{}{p}", s.addr.port())).unwrap();
    let key = "zai-test-key-5521";
    let w = web(
        &s,
        &[],
        10_000,
        Some(Backend::Zai {
            endpoint: base("/api/coding/paas/v4/web_search"),
            key: key.into(),
            engine: "search_pro_jina".into(),
        }),
    );
    let r = w
        .search(&open(), "Captain Comic 1988 PC game", 3)
        .await
        .unwrap();
    assert_eq!(r[0].title, "The Adventures of Captain Comic");
    assert!(r[0].site_only);
    let seen = s.seen().last().unwrap().clone();
    assert_eq!(seen.method, "POST");
    assert_eq!(seen.path, "/api/coding/paas/v4/web_search");
    assert_eq!(seen.headers["authorization"], format!("Bearer {key}"));
    assert!(seen.headers["content-type"].starts_with("application/json"));
    let body: serde_json::Value = serde_json::from_str(&seen.body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"search_engine": "search_pro_jina",
            "search_query": "Captain Comic 1988 PC game", "count": 3})
    );

    // Refusals and exhausted quotas are named; the reply's text is not shown.
    for (path, want) in [
        ("/quota/web_search", "rate limit or quota"),
        ("/denied/web_search", "key or the request was refused"),
    ] {
        let w = web(
            &s,
            &[],
            10_000,
            Some(Backend::Zai {
                endpoint: base(path),
                key: key.into(),
                engine: "search-prime".into(),
            }),
        );
        let e = w.search(&open(), "x", 3).await.unwrap_err();
        assert!(
            matches!(e, WebError::Search(ref m) if m.contains(want) && !m.contains("abc")),
            "{e}"
        );
        assert_eq!(e.outcome(), "search_error");
    }

    let w = web(
        &s,
        &[],
        10_000,
        Some(Backend::Wikipedia {
            endpoint: base("/w/api.php"),
        }),
    );
    let started = std::time::Instant::now();
    let r = w.search(&open(), "captain comic", 2).await.unwrap();
    let r2 = w.search(&open(), "captain comic game", 2).await.unwrap();
    // The second search waited for the first to be a second old.
    assert!(started.elapsed() >= declass_web::WIKIPEDIA_INTERVAL);
    assert_eq!(r, r2);
    assert_eq!(
        r[0].url,
        format!(
            "http://127.0.0.1:{}/wiki/The_Adventures_of_Captain_Comic",
            s.addr.port()
        )
    );
    assert_eq!(
        r[0].snippet,
        "a platform game released as shareware in 1988"
    );
    let seen = s.seen().last().unwrap().clone();
    assert_eq!(seen.method, "GET");
    assert!(
        seen.path.contains("srsearch=captain+comic+game") && seen.path.contains("list=search"),
        "{}",
        seen.path
    );
    // No key, and a User-Agent that names the software and where it lives.
    assert!(!seen.headers.contains_key("authorization"));
    let ua = &seen.headers["user-agent"];
    assert!(
        ua.starts_with("declass/") && ua.contains("+https://"),
        "{ua}"
    );
}

#[tokio::test]
async fn the_coding_plan_search_keeps_one_mcp_session_and_decodes_its_hits() {
    use declass_mcp::mock::{Answer, HttpMock, Mock};
    let hits = serde_json::json!([
        {"title": "The Adventures of Captain Comic", "link": "https://en.wikipedia.org/wiki/The_Adventures_of_Captain_Comic",
         "content": "a platform game written by Michael Denio", "refer": "ref_1"},
        {"title": "captain comic", "link": "https://en.namu.wiki", "content": "The first side-scrolling action game"}
    ]);
    let mock = Mock {
        // The server double-encodes: a JSON string holding the list.
        canned: serde_json::to_string(&hits.to_string()).unwrap(),
        ..Mock::default()
    };
    let server = HttpMock::start(mock.clone(), Answer::EventStream).await;
    let key = "zai-plan-key-7731";
    let w = Web::new(WebConfig {
        max_bytes: 100_000,
        timeout: Duration::from_secs(5),
        allowlist: Allowlist::default(),
        search: Some(Backend::ZaiPlan {
            endpoint: url::Url::parse(&server.url).unwrap(),
            key: key.into(),
        }),
    });
    let r = w.search(&open(), "what is Captain Comic", 5).await.unwrap();
    assert_eq!(r.len(), 2);
    assert!(!r[0].site_only && r[1].site_only);
    assert_eq!(
        w.search(&open(), "Captain Comic", 1).await.unwrap().len(),
        1
    );
    // One session: initialized once, two calls, the key in every request.
    let methods = mock.methods();
    assert_eq!(
        methods.iter().filter(|m| *m == "initialize").count(),
        1,
        "{methods:?}"
    );
    assert_eq!(
        mock.calls(),
        vec![
            serde_json::json!({"search_query": "what is Captain Comic", "location": "us"}),
            serde_json::json!({"search_query": "Captain Comic", "location": "us"}),
        ]
    );
    assert!(
        server
            .seen()
            .iter()
            .all(|s| s.authorization.as_deref() == Some(&format!("Bearer {key}")))
    );

    // A server that cannot be reached is a network error, not a crash.
    let w = Web::new(WebConfig {
        max_bytes: 100_000,
        timeout: Duration::from_secs(2),
        allowlist: Allowlist::default(),
        search: Some(Backend::ZaiPlan {
            endpoint: url::Url::parse("http://127.0.0.1:9/mcp").unwrap(),
            key: key.into(),
        }),
    });
    let e = w.search(&open(), "x", 3).await.unwrap_err();
    assert!(
        matches!(e, WebError::Network(_)) && !e.to_string().contains(key),
        "{e}"
    );
}

/// A real public page over HTTPS with the system resolver. Needs the internet,
/// so it runs only on request: `cargo test -p declass-web -- --ignored`.
#[tokio::test]
#[ignore = "needs the internet"]
async fn live_fetch_of_a_public_page() {
    let w = Web::new(WebConfig {
        max_bytes: 2_000_000,
        timeout: Duration::from_secs(30),
        allowlist: Allowlist::default(),
        search: None,
    });
    let page = w.fetch(&open(), "https://www.rust-lang.org").await.unwrap();
    assert_eq!(page.status, 200);
    assert!(page.text.contains("Rust"), "{}", page.text);
    assert!(page.text.contains("](https://"), "links are kept");
    println!(
        "{} {} bytes, {} lines of text",
        page.url,
        page.bytes,
        page.text.lines().count()
    );
    println!(
        "{}",
        page.text.lines().take(25).collect::<Vec<_>>().join("\n")
    );
}

fn live(search: Backend) -> Web {
    Web::new(WebConfig {
        max_bytes: 2_000_000,
        timeout: Duration::from_secs(30),
        allowlist: Allowlist::default(),
        search: Some(search),
    })
}

/// A real Wikipedia search. Needs the internet: `cargo test -p declass-web --
/// --ignored live_`.
#[tokio::test]
#[ignore = "needs the internet"]
async fn live_wikipedia_search() {
    let w = live(Backend::Wikipedia {
        endpoint: url::Url::parse(declass_web::search::WIKIPEDIA_ENDPOINT).unwrap(),
    });
    let s = w
        .search_with(&open(), "Captain Comic 1988 PC game", 5, None)
        .await
        .unwrap();
    print!(
        "{}",
        declass_web::search::render("Captain Comic 1988 PC game", &s)
    );
    let r = s.results;
    assert!(
        r.iter()
            .any(|x| x.url == "https://en.wikipedia.org/wiki/The_Adventures_of_Captain_Comic"),
        "{r:?}"
    );
}

/// A real Z.ai search with the key in `ZAI_API_KEY` (never printed): the
/// coding plan's search server, or with `DECLASS_ZAI_ENGINE` set, the Web Search
/// API with that engine (billed per search to the account's balance).
#[tokio::test]
#[ignore = "needs the internet and a Z.ai key"]
async fn live_zai_search() {
    let key = std::env::var("ZAI_API_KEY").expect("ZAI_API_KEY");
    let w = live(match std::env::var("DECLASS_ZAI_ENGINE") {
        Ok(engine) => Backend::Zai {
            endpoint: url::Url::parse(declass_web::search::ZAI_ENDPOINT).unwrap(),
            key,
            engine,
        },
        Err(_) => Backend::ZaiPlan {
            endpoint: url::Url::parse(declass_web::search::ZAI_PLAN_ENDPOINT).unwrap(),
            key,
        },
    });
    // The query in `DECLASS_LIVE_QUERY`, or the one a run should have asked.
    let query =
        std::env::var("DECLASS_LIVE_QUERY").unwrap_or_else(|_| "Captain Comic 1988 PC game".into());
    let s = w.search_with(&open(), &query, 5, None).await.unwrap();
    print!("{}", declass_web::search::render(&query, &s));
    // Relevance is the provider's; the test checks that hits arrive.
    assert!(!s.results.is_empty());
}

/// The guard of a run without the boundary: every request passes.
fn open() -> Guard {
    PassThrough { max_bytes: 0 }.outbound_guard()
}

/// Records every name it is asked to resolve; all resolve to `addr`.
struct Recording(SocketAddr, Mutex<Vec<String>>);

impl Resolve for Recording {
    fn resolve(
        &self,
        host: String,
        _port: u16,
    ) -> BoxFuture<'static, std::io::Result<Vec<SocketAddr>>> {
        self.1.lock().unwrap().push(host);
        let addr = self.0;
        Box::pin(async move { Ok(vec![addr]) })
    }
}

#[tokio::test]
async fn what_the_boundary_refuses_is_neither_resolved_nor_sent() {
    const PASSWORD: &str = "quartz-otter-5519";
    let dir = tempfile::tempdir().unwrap();
    let engine = declass_boundary::engine::Engine::open(
        dir.path(),
        declass_boundary::policy::Policy {
            sensitive_globs: vec![".env*".into()],
            ..Default::default()
        },
        None,
    )
    .unwrap();
    engine.present(
        &declass_boundary::view::Source::File {
            path: ".env".into(),
            ranged: false,
        },
        format!("DB_PASSWORD={PASSWORD}\n").as_bytes(),
    );
    let guard = engine.outbound_guard();
    let s = serve(HashMap::from([
        ("/", reply(200, "text/plain", "hi")),
        ("/go", redirect(&format!("/leak?k={PASSWORD}"))),
        (
            "/searx/search",
            reply(200, "application/json", r#"{"results":[]}"#),
        ),
    ]))
    .await;
    let names = Arc::new(Recording(s.addr, Mutex::new(Vec::new())));
    let searx = url::Url::parse(&format!("http://127.0.0.1:{}/searx", s.addr.port())).unwrap();
    let w = Web::new(WebConfig {
        max_bytes: 10_000,
        timeout: Duration::from_secs(2),
        allowlist: Allowlist::parse(&["127.0.0.1".to_owned()]).unwrap(),
        search: Some(Backend::Searxng { base: searx }),
    })
    .with_resolver(names.clone());
    let port = s.addr.port();
    for url in [
        format!("http://{PASSWORD}.public.test:{port}/"),
        format!("http://public.test:{port}/?k={}", PASSWORD.to_uppercase()),
    ] {
        let e = w.fetch(&guard, &url).await.unwrap_err();
        assert!(matches!(e, WebError::NotSent(_)), "{url}: {e:?}");
        assert_eq!(e.outcome(), "refused_outbound");
    }
    assert!(
        names.1.lock().unwrap().is_empty(),
        "a refused name was resolved"
    );
    assert!(s.seen().is_empty(), "a refused request was sent");
    // The server's redirect to a URL holding the value is a new request: refused too.
    let e = w
        .fetch(&guard, &format!("http://public.test:{port}/go"))
        .await
        .unwrap_err();
    assert!(matches!(e, WebError::NotSent(_)), "{e:?}");
    assert_eq!(s.seen().len(), 1);
    // A search query holding it never reaches the backend.
    let e = w
        .search(&guard, &format!("password {PASSWORD}"), 3)
        .await
        .unwrap_err();
    assert!(matches!(e, WebError::NotSent(_)), "{e:?}");
    assert_eq!(s.seen().len(), 1);
    assert!(
        !s.seen()
            .iter()
            .any(|r| r.path.contains(PASSWORD) || r.body.contains(PASSWORD))
    );
    // Each refusal is recorded for the audit log, without the value.
    let refused: Vec<_> = engine
        .take_events()
        .into_iter()
        .filter_map(|e| match e {
            declass_boundary::audit::AuditEvent::OutboundRefused { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(refused.len(), 4, "{refused:?}");
    assert!(refused.iter().all(|r| !r.contains(PASSWORD)), "{refused:?}");
}
