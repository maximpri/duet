// SPDX-License-Identifier: GPL-3.0-or-later
//! Search backends: native (the host asks public sources with open APIs
//! itself, no search provider in between: [`native`]; the default), a
//! self-hosted SearXNG instance (JSON API, no key), the Brave Search API (key
//! from an environment variable), Wikipedia alone (the MediaWiki search API,
//! no key) and Z.ai's web search (with the frontier's key: the GLM Coding
//! Plan's search server, or the per-search Web Search API).

pub mod native;

use native::NativeSearch;
use serde_json::{Map, Value, json};
use url::Url;

/// The Brave Search web endpoint.
pub const BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
/// Z.ai's Web Search API: billed per search to the account's balance, not
/// covered by the GLM Coding Plan.
pub const ZAI_ENDPOINT: &str = "https://api.z.ai/api/paas/v4/web_search";
/// The API's engines, the default first. `search_pro_jina` gave page
/// addresses and relevant hits in live tests (2026-09-26); `search-prime`
/// (the one the API schema lists) often gave only a hit's site, and unrelated
/// hits for technical queries.
pub const ZAI_ENGINES: &[&str] = &["search_pro_jina", "search-prime"];
/// The GLM Coding Plan's Web Search server (MCP over streamable HTTP; each
/// search counts against the plan's credits).
pub const ZAI_PLAN_ENDPOINT: &str = "https://api.z.ai/api/mcp/web_search_prime/mcp";
/// Its one tool.
pub const ZAI_PLAN_TOOL: &str = "web_search_prime";
/// English Wikipedia's MediaWiki Action API.
pub const WIKIPEDIA_ENDPOINT: &str = "https://en.wikipedia.org/w/api.php";
/// Snippets longer than this are cut: results point at pages, they are not
/// the pages.
pub const MAX_SNIPPET_CHARS: usize = 600;

/// One search hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// The backend gave only the site (`https://host/`), not the page's address.
    pub site_only: bool,
    /// Where it came from: the backend's name, or the native source's.
    pub source: &'static str,
}

/// What one source (or a single backend) did for one search: for the audit
/// log (host, bytes, outcome; never the query) and the frontier (note).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReport {
    pub source: &'static str,
    pub host: String,
    /// Bytes of the reply.
    pub bytes: usize,
    /// `ok`, `cached`, `not_applicable`, `throttled`, `backoff`,
    /// `rate_limited`, or a failure ([`crate::WebError::outcome`]).
    pub outcome: &'static str,
    /// Why it was skipped or failed, for the frontier (never the reply's text).
    pub note: String,
    /// Results it gave.
    pub hits: usize,
}

/// A search's results and what each source did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Searched {
    pub results: Vec<SearchResult>,
    pub requests: Vec<SourceReport>,
    /// Results come from several sources and are shown with their source.
    pub labelled: bool,
}

/// Where searches go. Keys are held only in memory and never printed.
#[derive(Clone)]
pub enum Backend {
    /// Z.ai's Web Search API with one of [`ZAI_ENGINES`].
    Zai {
        endpoint: Url,
        key: String,
        engine: String,
    },
    /// The GLM Coding Plan's search server.
    ZaiPlan {
        endpoint: Url,
        key: String,
    },
    Searxng {
        base: Url,
    },
    Brave {
        endpoint: Url,
        key: String,
    },
    Wikipedia {
        endpoint: Url,
    },
    /// Public sources asked by the host itself.
    Native(NativeSearch),
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Backend::Zai {
                endpoint, engine, ..
            } => write!(f, "Zai({endpoint}, {engine}, key withheld)"),
            Backend::ZaiPlan { endpoint, .. } => write!(f, "ZaiPlan({endpoint}, key withheld)"),
            Backend::Searxng { base } => write!(f, "Searxng({base})"),
            Backend::Brave { endpoint, .. } => write!(f, "Brave({endpoint}, key withheld)"),
            Backend::Wikipedia { endpoint } => write!(f, "Wikipedia({endpoint})"),
            Backend::Native(n) => {
                let names: Vec<&str> = n.sources.iter().map(|s| s.source.name()).collect();
                write!(f, "Native({})", names.join(", "))
            }
        }
    }
}

/// How one search is made.
pub(crate) enum Call {
    /// A `GET`, or a `POST` of a JSON body; the reply is JSON.
    Http {
        url: Url,
        headers: Vec<(&'static str, String)>,
        body: Option<Value>,
    },
    /// A `tools/call` on an MCP server over streamable HTTP; the result is
    /// text holding JSON.
    Mcp {
        url: Url,
        headers: Vec<(String, String)>,
        tool: &'static str,
        arguments: Map<String, Value>,
    },
}

impl Backend {
    pub fn name(&self) -> &'static str {
        match self {
            Backend::Zai { .. } => "zai",
            Backend::ZaiPlan { .. } => "zai (coding plan)",
            Backend::Searxng { .. } => "searxng",
            Backend::Brave { .. } => "brave",
            Backend::Wikipedia { .. } => "wikipedia",
            Backend::Native(_) => "native",
        }
    }

    /// The host queries are sent to (for outbound checks and the audit log);
    /// for the native backend, the hosts of its default sources.
    pub fn host(&self) -> String {
        match self {
            Backend::Zai { endpoint, .. }
            | Backend::ZaiPlan { endpoint, .. }
            | Backend::Brave { endpoint, .. }
            | Backend::Wikipedia { endpoint } => endpoint.host_str().unwrap_or_default().to_owned(),
            Backend::Searxng { base } => base.host_str().unwrap_or_default().to_owned(),
            Backend::Native(n) => NativeSearch::hosts(&n.defaults().collect::<Vec<_>>()).join(", "),
        }
    }

    /// Whether this backend searches the web at large (not a set of sources).
    pub fn whole_web(&self) -> bool {
        !matches!(self, Backend::Wikipedia { .. } | Backend::Native(_))
    }

    /// Results a search returns when the frontier gives no count.
    pub fn default_count(&self) -> usize {
        match self {
            Backend::Native(_) => native::DEFAULT_RESULTS,
            _ => crate::DEFAULT_RESULTS,
        }
    }

    /// How to search for `query` (`None` for the native backend, which asks
    /// each of its sources itself).
    pub(crate) fn call(&self, query: &str, count: usize) -> Option<Call> {
        let get = |url: Url| Call::Http {
            url,
            headers: Vec::new(),
            body: None,
        };
        Some(match self {
            Backend::Zai {
                endpoint,
                key,
                engine,
            } => Call::Http {
                url: endpoint.clone(),
                headers: vec![("Authorization", format!("Bearer {key}"))],
                body: Some(json!({
                    "search_engine": engine,
                    "search_query": query,
                    "count": count,
                })),
            },
            Backend::ZaiPlan { endpoint, key } => {
                let mut arguments = Map::new();
                arguments.insert("search_query".into(), query.into());
                // The server ranks for Chinese readers unless told otherwise.
                let region = if query.chars().any(is_cjk) {
                    "cn"
                } else {
                    "us"
                };
                arguments.insert("location".into(), region.into());
                Call::Mcp {
                    url: endpoint.clone(),
                    headers: vec![("Authorization".into(), format!("Bearer {key}"))],
                    tool: ZAI_PLAN_TOOL,
                    arguments,
                }
            }
            Backend::Searxng { base } => {
                let mut url = base.clone();
                if !url.path().ends_with('/') {
                    let p = format!("{}/", url.path());
                    url.set_path(&p);
                }
                let mut url = url.join("search").unwrap_or_else(|_| base.clone());
                url.query_pairs_mut()
                    .append_pair("q", query)
                    .append_pair("format", "json");
                get(url)
            }
            Backend::Brave { endpoint, key } => {
                let mut url = endpoint.clone();
                url.query_pairs_mut()
                    .append_pair("q", query)
                    .append_pair("count", &count.to_string());
                Call::Http {
                    url,
                    headers: vec![("X-Subscription-Token", key.clone())],
                    body: None,
                }
            }
            Backend::Wikipedia { endpoint } => {
                let mut url = endpoint.clone();
                url.query_pairs_mut()
                    .append_pair("action", "query")
                    .append_pair("list", "search")
                    .append_pair("srsearch", query)
                    .append_pair("srlimit", &count.to_string())
                    .append_pair("srprop", "snippet")
                    .append_pair("format", "json")
                    .append_pair("formatversion", "2");
                get(url)
            }
            Backend::Native(_) => return None,
        })
    }

    /// What a refused request means, from the reply's status and body (the
    /// body itself is never shown).
    pub(crate) fn refusal(&self, status: u16, body: &[u8]) -> &'static str {
        let code = serde_json::from_slice::<Value>(body).ok().and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("code"))
                .map(|c| c.as_str().map_or_else(|| c.to_string(), str::to_owned))
        });
        match (self, status, code.as_deref()) {
            (Backend::Zai { .. }, _, Some("1113")) => {
                " (the Z.ai account has no balance for the Web Search API, which is billed per search \
and not by the coding plan; web.search.zai_engine = \"plan\" searches within the plan)"
            }
            (_, 401 | 403, _) => " (the key or the request was refused)",
            (_, 429, _) => " (rate limit or quota reached; try again later)",
            _ => "",
        }
    }

    /// Parses a backend's JSON reply into at most `count` results.
    pub fn parse(&self, body: &Value, count: usize) -> Result<Vec<SearchResult>, String> {
        let (list, url_key, snippet_key) = match self {
            Backend::Zai { .. } | Backend::ZaiPlan { .. } => {
                (body.get("search_result"), "link", "content")
            }
            Backend::Searxng { .. } => (body.get("results"), "url", "content"),
            Backend::Brave { .. } => (
                body.get("web").and_then(|w| w.get("results")),
                "url",
                "description",
            ),
            Backend::Wikipedia { .. } => (
                body.get("query").and_then(|q| q.get("search")),
                "title",
                "snippet",
            ),
            Backend::Native(_) => return Err("the native backend reads each source's reply".into()),
        };
        let Some(list) = list.filter(|l| !l.is_null()) else {
            // Brave answers a query without web hits with no `web` section,
            // Z.ai sometimes with no result list.
            let empty = match self {
                Backend::Brave { .. } => body.get("type").is_some(),
                Backend::Zai { .. } => body.get("id").is_some() && body.get("error").is_none(),
                _ => false,
            };
            if empty {
                return Ok(Vec::new());
            }
            return Err(format!("the {} reply has no results list", self.name()));
        };
        let list = list
            .as_array()
            .ok_or_else(|| format!("the {} results are not a list", self.name()))?;
        let text = |v: &Value, k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .map(clean)
                .unwrap_or_default()
        };
        let zai = matches!(self, Backend::Zai { .. } | Backend::ZaiPlan { .. });
        Ok(list
            .iter()
            .filter_map(|r| {
                let raw = r.get(url_key).and_then(Value::as_str)?;
                let url = match self {
                    Backend::Wikipedia { endpoint } => article_url(endpoint, raw)?,
                    _ => raw.to_owned(),
                };
                Some(SearchResult {
                    title: text(r, "title"),
                    site_only: zai && is_site_root(&url),
                    url,
                    snippet: cut(text(r, snippet_key)),
                    source: self.name(),
                })
            })
            .take(count)
            .collect())
    }

    /// Parses an MCP tool's text result: the plan's server answers with the
    /// hits as a JSON list, itself encoded as a JSON string.
    pub fn parse_text(&self, text: &str, count: usize) -> Result<Vec<SearchResult>, String> {
        let mut v: Value = serde_json::from_str(text.trim())
            .map_err(|_| format!("the {} result is not JSON", self.name()))?;
        if let Value::String(inner) = &v {
            v = serde_json::from_str(inner)
                .map_err(|_| format!("the {} result is not a JSON list", self.name()))?;
        }
        self.parse(&json!({ "search_result": v }), count)
    }
}

/// Chinese, Japanese and Korean scripts.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xac00..=0xd7af | 0xf900..=0xfaff)
}

/// The article URL of a Wikipedia title, on the API endpoint's host.
fn article_url(endpoint: &Url, title: &str) -> Option<String> {
    if title.trim().is_empty() {
        return None;
    }
    let mut url = endpoint.join("/wiki/").ok()?;
    url.set_query(None);
    {
        let mut segments = url.path_segments_mut().ok()?;
        segments.pop_if_empty();
        // Subpage slashes stay slashes; everything else is percent-encoded.
        for part in title.trim().replace(' ', "_").split('/') {
            segments.push(part);
        }
    }
    Some(url.to_string())
}

/// Whether `url` is only a site (`https://host/`): Z.ai's search often gives
/// the site of a hit rather than its page.
fn is_site_root(url: &str) -> bool {
    Url::parse(url).is_ok_and(|u| {
        matches!(u.path(), "" | "/") && u.query().is_none() && u.fragment().is_none()
    })
}

/// Snippets carry highlighting markup (`<strong>`, `<span class="searchmatch">`)
/// and entities.
pub(crate) fn clean(s: &str) -> String {
    crate::html::to_text(s, None)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn cut(s: String) -> String {
    match s.char_indices().nth(MAX_SNIPPET_CHARS) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s,
    }
}

/// What a source did, in a few words.
fn status(r: &SourceReport) -> String {
    let hits = |n: usize| match n {
        0 => "no results".to_owned(),
        1 => "1 result".to_owned(),
        n => format!("{n} results"),
    };
    match r.outcome {
        "ok" if r.note.is_empty() => format!("{} ({})", r.source, hits(r.hits)),
        "ok" => format!("{} ({}; {})", r.source, hits(r.hits), r.note),
        "cached" => format!("{} ({}, answered earlier)", r.source, hits(r.hits)),
        "not_applicable" | "throttled" | "backoff" => format!("{} (skipped: {})", r.source, r.note),
        _ => format!("{} ({})", r.source, r.note),
    }
}

/// Search results as the frontier reads them: for several sources, first
/// what each source did, then the results with their source.
pub fn render(query: &str, searched: &Searched) -> String {
    let results = &searched.results;
    let mut out = String::new();
    if searched.labelled {
        let asked: Vec<String> = searched.requests.iter().map(status).collect();
        out.push_str(&format!("Sources asked: {}.\n", asked.join("; ")));
    }
    if results.is_empty() {
        out.push_str(&format!("No results for {query:?}.\n"));
        return out;
    }
    out.push_str(&format!("Results for {query:?}:\n"));
    for (i, r) in results.iter().enumerate() {
        let note = if r.site_only {
            " (the site only; the search gave no page address)"
        } else {
            ""
        };
        let from = if searched.labelled {
            format!(" [{}]", r.source)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "{}. {}{from}\n   {}{note}\n",
            i + 1,
            r.title,
            r.url
        ));
        if !r.snippet.is_empty() {
            out.push_str(&format!("   {}\n", r.snippet));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    type Http = (Url, Vec<(&'static str, String)>, Option<Value>);

    fn render(query: &str, results: &[SearchResult]) -> String {
        super::render(
            query,
            &Searched {
                results: results.to_vec(),
                requests: Vec::new(),
                labelled: false,
            },
        )
    }

    fn http(call: Option<Call>) -> Http {
        match call.expect("a call") {
            Call::Http { url, headers, body } => (url, headers, body),
            Call::Mcp { .. } => panic!("an MCP call"),
        }
    }

    #[test]
    fn searxng_and_brave_parse_title_url_and_snippet() {
        let searx = Backend::Searxng {
            base: Url::parse("http://127.0.0.1:8888").unwrap(),
        };
        let body = json!({"query": "rust", "results": [
            {"title": "Rust", "url": "https://www.rust-lang.org/", "content": "A language &amp; more", "engine": "x"},
            {"title": "No url"},
            {"title": "Book", "url": "https://doc.rust-lang.org/book/", "content": ""}
        ]});
        let r = searx.parse(&body, 5).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].snippet, "A language & more");
        // Only Z.ai's site-only links are marked.
        assert!(!r[0].site_only);
        assert_eq!(searx.parse(&body, 1).unwrap().len(), 1);
        let (url, headers, body) = http(searx.call("a b", 5));
        assert_eq!(
            url.as_str(),
            "http://127.0.0.1:8888/search?q=a+b&format=json"
        );
        assert!(headers.is_empty() && body.is_none());

        let brave = Backend::Brave {
            endpoint: Url::parse(BRAVE_ENDPOINT).unwrap(),
            key: "k-secret".into(),
        };
        let body = json!({"type": "search", "web": {"results": [
            {"title": "The <strong>Rust</strong> Book", "url": "https://doc.rust-lang.org/book/",
             "description": "Learn <strong>Rust</strong>"}
        ]}});
        let r = brave.parse(&body, 5).unwrap();
        assert_eq!(r[0].title, "The Rust Book");
        assert_eq!(r[0].snippet, "Learn Rust");
        assert!(
            brave
                .parse(&json!({"type": "search"}), 5)
                .unwrap()
                .is_empty()
        );
        assert!(brave.parse(&json!({"error": "x"}), 5).is_err());
        assert!(!format!("{brave:?}").contains("k-secret"));
        let text = render("rust", &r);
        assert!(
            text.contains("1. The Rust Book\n   https://doc.rust-lang.org/book/\n   Learn Rust")
        );
    }

    #[test]
    fn zai_posts_the_documented_body_and_marks_site_only_links() {
        let zai = Backend::Zai {
            endpoint: Url::parse(ZAI_ENDPOINT).unwrap(),
            key: "zai-secret".into(),
            engine: "search-prime".into(),
        };
        let (url, headers, body) = http(zai.call("Captain Comic 1988", 4));
        assert_eq!(url.as_str(), ZAI_ENDPOINT);
        assert_eq!(
            body.unwrap(),
            json!({"search_engine": "search-prime", "search_query": "Captain Comic 1988", "count": 4})
        );
        assert_eq!(
            headers,
            vec![("Authorization", "Bearer zai-secret".to_owned())]
        );
        assert!(!format!("{zai:?}").contains("zai-secret"));
        assert_eq!(zai.host(), "api.z.ai");

        // The shape of a live reply (2026-09-26), shortened.
        let body = json!({"created": 1790383976, "id": "2026092608", "search_result": [
            {"content": "The Adventures of Captain Comic is a platform game written by Michael Denio",
             "icon": "", "link": "https://en.wikipedia.org", "media": "", "publish_date": "",
             "refer": "ref_1", "title": "The Adventures of Captain Comic"},
            {"content": "Longplay", "link": "https://www.youtube.com/watch?v=alSOnAnO7yo",
             "title": "Longplay: Captain Comic (1988)"},
            {"title": "no link"}
        ]});
        let r = zai.parse(&body, 10).unwrap();
        assert_eq!(r.len(), 2);
        assert!(r[0].site_only && !r[1].site_only);
        let text = render("captain comic", &r);
        assert!(
            text.contains(
                "https://en.wikipedia.org (the site only; the search gave no page address)"
            ),
            "{text}"
        );
        assert!(
            zai.parse(&json!({"id": "x", "search_result": null}), 5)
                .unwrap()
                .is_empty()
        );
        assert!(
            zai.parse(
                &json!({"error": {"code": "1113", "message": "no balance"}}),
                5
            )
            .is_err()
        );
    }

    #[test]
    fn the_coding_plan_is_asked_through_its_tool_and_its_text_is_decoded() {
        let plan = Backend::ZaiPlan {
            endpoint: Url::parse(ZAI_PLAN_ENDPOINT).unwrap(),
            key: "zai-secret".into(),
        };
        assert!(!format!("{plan:?}").contains("zai-secret"));
        let Call::Mcp {
            url,
            headers,
            tool,
            arguments,
        } = plan.call("what is Captain Comic", 5).expect("a call")
        else {
            panic!("not an MCP call");
        };
        assert_eq!(url.as_str(), ZAI_PLAN_ENDPOINT);
        assert_eq!(tool, "web_search_prime");
        assert_eq!(
            headers,
            vec![("Authorization".to_owned(), "Bearer zai-secret".to_owned())]
        );
        assert_eq!(
            Value::Object(arguments),
            json!({"search_query": "what is Captain Comic", "location": "us"})
        );
        let Call::Mcp { arguments, .. } = plan.call("船长漫画 游戏", 5).expect("a call")
        else {
            panic!("not an MCP call");
        };
        assert_eq!(arguments["location"], "cn");

        // The live server's text (2026-09-26): a JSON string holding the list.
        let hits = json!([
            {"title": "captain comic", "link": "https://en.namu.wiki", "content": "The first side-scrolling action game", "refer": "ref_1"},
            {"title": "The Adventures of Captain Comic", "link": "https://en.wikipedia.org/wiki/The_Adventures_of_Captain_Comic", "content": "A platform game"}
        ]);
        let text = serde_json::to_string(&hits.to_string()).unwrap();
        let r = plan.parse_text(&text, 5).unwrap();
        assert_eq!(r.len(), 2);
        assert!(r[0].site_only && !r[1].site_only);
        assert_eq!(plan.parse_text(&hits.to_string(), 1).unwrap().len(), 1);
        assert!(plan.parse_text("quota exceeded", 5).is_err());
        assert!(plan.parse_text("\"not a list\"", 5).is_err());
    }

    #[test]
    fn refusals_are_explained_without_the_reply() {
        let zai = Backend::Zai {
            endpoint: Url::parse(ZAI_ENDPOINT).unwrap(),
            key: "k".into(),
            engine: "search_pro_jina".into(),
        };
        let balance = br#"{"error":{"code":"1113","message":"Insufficient balance or no resource package. Please recharge."}}"#;
        let why = zai.refusal(429, balance);
        assert!(
            why.contains("billed per search") && !why.contains("recharge"),
            "{why}"
        );
        assert!(zai.refusal(429, b"{}").contains("rate limit"));
        assert!(zai.refusal(401, b"").contains("refused"));
        assert_eq!(zai.refusal(500, b"oops"), "");
        let wiki = Backend::Wikipedia {
            endpoint: Url::parse(WIKIPEDIA_ENDPOINT).unwrap(),
        };
        assert!(!wiki.refusal(429, balance).contains("billed"));
    }

    #[test]
    fn wikipedia_results_link_to_their_articles() {
        let wiki = Backend::Wikipedia {
            endpoint: Url::parse(WIKIPEDIA_ENDPOINT).unwrap(),
        };
        let (url, headers, body) = http(wiki.call("captain comic", 3));
        assert!(body.is_none() && headers.is_empty());
        let q: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        for pair in [
            ("action", "query"),
            ("list", "search"),
            ("srsearch", "captain comic"),
            ("srlimit", "3"),
            ("format", "json"),
        ] {
            assert!(q.contains(&(pair.0.into(), pair.1.into())), "{q:?}");
        }
        let body = json!({"batchcomplete": true, "query": {"searchinfo": {"totalhits": 353}, "search": [
            {"ns": 0, "title": "The Adventures of Captain Comic", "pageid": 1558412,
             "snippet": "Adventures of <span class=\"searchmatch\">Captain</span> <span class=\"searchmatch\">Comic</span> is a platform game"},
            {"ns": 0, "title": "AC/DC", "snippet": "rock band"},
            {"ns": 0, "title": "Café 100% (film)", "snippet": ""}
        ]}});
        let r = wiki.parse(&body, 5).unwrap();
        assert_eq!(
            r[0].url,
            "https://en.wikipedia.org/wiki/The_Adventures_of_Captain_Comic"
        );
        assert_eq!(
            r[0].snippet,
            "Adventures of Captain Comic is a platform game"
        );
        assert_eq!(r[1].url, "https://en.wikipedia.org/wiki/AC/DC");
        assert_eq!(
            r[2].url,
            "https://en.wikipedia.org/wiki/Caf%C3%A9_100%25_(film)"
        );
        assert!(r.iter().all(|x| !x.site_only));
        assert!(
            wiki.parse(&json!({"query": {"search": []}}), 5)
                .unwrap()
                .is_empty()
        );
        assert!(
            wiki.parse(&json!({"error": {"code": "badvalue"}}), 5)
                .is_err()
        );
    }

    #[test]
    fn long_snippets_are_cut() {
        let long = "word ".repeat(400);
        let s = cut(clean(&long));
        assert_eq!(s.chars().count(), MAX_SNIPPET_CHARS + 1);
        assert!(s.ends_with('…'));
        assert_eq!(cut("short".into()), "short");
    }
}
