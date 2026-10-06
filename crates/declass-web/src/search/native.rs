// SPDX-License-Identifier: GPL-3.0-or-later
//! The native search backend: the host itself asks public sources that
//! publish an API for automated use, in parallel, and merges their answers.
//! No search provider sits in between, so every source asked receives the
//! query (and the host's address).
//!
//! Sources, and the terms each is used under (checked 2026-09-26 in the
//! operators' own documentation):
//! - `wikipedia`: the MediaWiki Action API. Unauthenticated clients keep to
//!   one request at a time and under 5 a second (Wikimedia robot policy).
//! - `stackoverflow`, `serverfault`, `superuser`, `askubuntu`, `unix`: the
//!   Stack Exchange API 2.3 (`search/advanced`). Without a key, 300 requests
//!   a day per IP address; a reply's `backoff` must be waited out; replies are
//!   always compressed; results must name Stack Exchange as the source (the
//!   source name and link do).
//! - `github`, `github_issues`: GitHub's REST search for repositories and for
//!   issues and pull requests. Unauthenticated: 10 searches a minute per IP
//!   address. Never sent with a token.
//! - `crates`: the crates.io API: at most one request a second, with a
//!   User-Agent naming the client and how to reach its authors.
//! - `npm`: the registry's documented `/-/v1/search`.
//! - `pypi`: PyPI's JSON API, exact project names only: PyPI has no search
//!   API (its XML-RPC search is disabled).
//! - `hackernews`: Algolia's Hacker News Search API (10,000 requests an hour
//!   per IP address).
//! - `arxiv`: the arXiv API (Atom): one request every three seconds, one
//!   connection at a time.
//!
//! Not asked: the result pages of general search engines (Google, Bing,
//! DuckDuckGo and the like). They publish no API for this without an account
//! and a contract, their terms forbid automated querying of the pages, and
//! scraped markup breaks without notice. MDN's site search (its `robots.txt`
//! disallows `/api/`, and the endpoint is not documented for others) and
//! docs.rs (no search API; crates.io results link to the documentation) are
//! left out for the same reason.
//!
//! Politeness: one request at a time per service with its interval, GitHub's
//! per-minute budget counted locally, a `429` (or GitHub's exhausted budget,
//! or Stack Exchange's `backoff` and spent quota) waited out for that source
//! only, answers kept for the run, and a per-source timeout so a slow source
//! never holds up the others. A source that would have to wait is skipped and
//! named as such.

use super::{SearchResult, Searched, SourceReport, clean, cut};
use crate::{Web, WebError};
use declass_boundary::third_party::{Guard, Outgoing};
use declass_net::{Checked, Credentials};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use url::Url;

/// Longest time one source may take, the wait for its turn included
/// (`web.timeout_secs` when that is shorter).
pub const SOURCE_TIMEOUT: Duration = Duration::from_secs(10);
/// Results a native search returns when no count is given: a few per source.
pub const DEFAULT_RESULTS: usize = 8;
/// GitHub's search queries are at most this long (qualifiers aside).
pub const GITHUB_MAX_QUERY: usize = 256;
/// Answers kept per run (source, query and count); later ones are not kept.
const CACHE_ENTRIES: usize = 256;
/// The first wait after a `429` without `Retry-After`; it doubles on each
/// further one, up to [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);
/// How long a spent Stack Exchange quota keeps the source quiet (the quota is
/// daily; the run is usually over before it returns).
const QUOTA_BACKOFF: Duration = Duration::from_secs(60 * 60);
/// Longest body of an issue shown in a snippet.
const ISSUE_BODY_CHARS: usize = 300;

/// A public source the native backend can ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    StackOverflow,
    ServerFault,
    SuperUser,
    AskUbuntu,
    Unix,
    Wikipedia,
    GitHub,
    GitHubIssues,
    Crates,
    Npm,
    PyPi,
    HackerNews,
    Arxiv,
}

/// Every source, in the order results are ranked (a source's first result
/// before any source's second): answers to questions first, then what a
/// thing is, then projects and packages, then discussions and papers.
pub const ALL: &[Source] = &[
    Source::StackOverflow,
    Source::ServerFault,
    Source::SuperUser,
    Source::AskUbuntu,
    Source::Unix,
    Source::Wikipedia,
    Source::GitHub,
    Source::GitHubIssues,
    Source::Crates,
    Source::Npm,
    Source::PyPi,
    Source::HackerNews,
    Source::Arxiv,
];

/// Who runs a source: sources of one service share its limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Service {
    Wikimedia,
    StackExchange,
    GitHub,
    Crates,
    Npm,
    PyPi,
    HackerNews,
    Arxiv,
}

impl Service {
    /// Least time between the end of one request and the start of the next.
    fn interval(self) -> Duration {
        match self {
            Service::Arxiv => Duration::from_secs(3),
            Service::GitHub => Duration::ZERO,
            _ => Duration::from_secs(1),
        }
    }

    /// At most this many requests in this long (GitHub's search budget).
    fn window(self) -> Option<(usize, Duration)> {
        match self {
            Service::GitHub => Some((10, Duration::from_secs(60))),
            _ => None,
        }
    }
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::StackOverflow => "stackoverflow",
            Source::ServerFault => "serverfault",
            Source::SuperUser => "superuser",
            Source::AskUbuntu => "askubuntu",
            Source::Unix => "unix",
            Source::GitHub => "github",
            Source::GitHubIssues => "github_issues",
            Source::Crates => "crates",
            Source::Npm => "npm",
            Source::PyPi => "pypi",
            Source::Wikipedia => "wikipedia",
            Source::HackerNews => "hackernews",
            Source::Arxiv => "arxiv",
        }
    }

    pub fn from_name(name: &str) -> Option<Source> {
        ALL.iter().copied().find(|s| s.name() == name.trim())
    }

    /// What it holds, for the tool's description.
    pub fn about(self) -> &'static str {
        match self {
            Source::StackOverflow => "Stack Overflow questions",
            Source::ServerFault => "Server Fault questions (system administration)",
            Source::SuperUser => "Super User questions (computer use)",
            Source::AskUbuntu => "Ask Ubuntu questions",
            Source::Unix => "Unix & Linux Stack Exchange questions",
            Source::GitHub => "GitHub repositories",
            Source::GitHubIssues => "GitHub issues and pull requests (error messages, bug reports)",
            Source::Crates => "crates.io Rust packages",
            Source::Npm => "npm JavaScript packages",
            Source::PyPi => "PyPI Python packages, by exact name only (a one-word query)",
            Source::Wikipedia => "English Wikipedia articles",
            Source::HackerNews => "Hacker News stories",
            Source::Arxiv => "arXiv papers",
        }
    }

    /// The source's API endpoint.
    pub fn endpoint(self) -> Url {
        let raw = match self {
            Source::StackOverflow
            | Source::ServerFault
            | Source::SuperUser
            | Source::AskUbuntu
            | Source::Unix => "https://api.stackexchange.com/2.3/search/advanced",
            Source::GitHub => "https://api.github.com/search/repositories",
            Source::GitHubIssues => "https://api.github.com/search/issues",
            Source::Crates => "https://crates.io/api/v1/crates",
            Source::Npm => "https://registry.npmjs.org/-/v1/search",
            Source::PyPi => "https://pypi.org/pypi/",
            Source::Wikipedia => crate::search::WIKIPEDIA_ENDPOINT,
            Source::HackerNews => "https://hn.algolia.com/api/v1/search",
            Source::Arxiv => "https://export.arxiv.org/api/query",
        };
        Url::parse(raw).expect("a valid constant")
    }

    fn service(self) -> Service {
        match self {
            Source::StackOverflow
            | Source::ServerFault
            | Source::SuperUser
            | Source::AskUbuntu
            | Source::Unix => Service::StackExchange,
            Source::GitHub | Source::GitHubIssues => Service::GitHub,
            Source::Crates => Service::Crates,
            Source::Npm => Service::Npm,
            Source::PyPi => Service::PyPi,
            Source::Wikipedia => Service::Wikimedia,
            Source::HackerNews => Service::HackerNews,
            Source::Arxiv => Service::Arxiv,
        }
    }

    /// The Stack Exchange site a source searches.
    fn site(self) -> Option<&'static str> {
        Some(match self {
            Source::StackOverflow => "stackoverflow",
            Source::ServerFault => "serverfault",
            Source::SuperUser => "superuser",
            Source::AskUbuntu => "askubuntu",
            Source::Unix => "unix",
            _ => return None,
        })
    }

    /// The request for `query`, or why this source does not take it.
    pub(crate) fn request(
        self,
        endpoint: &Url,
        query: &str,
        count: usize,
    ) -> Result<Url, &'static str> {
        let mut url = endpoint.clone();
        let n = count.to_string();
        if let Some(site) = self.site() {
            url.query_pairs_mut()
                .append_pair("order", "desc")
                .append_pair("sort", "relevance")
                .append_pair("q", query)
                .append_pair("site", site)
                .append_pair("pagesize", &n)
                .append_pair("filter", "default");
            return Ok(url);
        }
        match self {
            Source::GitHub | Source::GitHubIssues => {
                if query.chars().count() > GITHUB_MAX_QUERY {
                    return Err("GitHub takes queries of at most 256 characters");
                }
                url.query_pairs_mut()
                    .append_pair("q", query)
                    .append_pair("per_page", &n);
            }
            Source::Crates => {
                url.query_pairs_mut()
                    .append_pair("q", query)
                    .append_pair("per_page", &n);
            }
            Source::Npm => {
                url.query_pairs_mut()
                    .append_pair("text", query)
                    .append_pair("size", &n);
            }
            Source::PyPi => {
                let name = pypi_name(query).ok_or("PyPI is asked for one exact package name")?;
                if !url.path().ends_with('/') {
                    let p = format!("{}/", url.path());
                    url.set_path(&p);
                }
                url = url
                    .join(&format!("{name}/json"))
                    .map_err(|_| "not a package name")?;
            }
            Source::Wikipedia => {
                url.query_pairs_mut()
                    .append_pair("action", "query")
                    .append_pair("list", "search")
                    .append_pair("srsearch", query)
                    .append_pair("srlimit", &n)
                    .append_pair("srprop", "snippet")
                    .append_pair("format", "json")
                    .append_pair("formatversion", "2");
            }
            Source::HackerNews => {
                url.query_pairs_mut()
                    .append_pair("query", query)
                    .append_pair("tags", "story")
                    .append_pair("hitsPerPage", &n);
            }
            Source::Arxiv => {
                let words: Vec<String> = query
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|w| !w.is_empty())
                    .take(10)
                    .map(|w| format!("all:{w}"))
                    .collect();
                if words.is_empty() {
                    return Err("arXiv is asked for words");
                }
                url.query_pairs_mut()
                    .append_pair("search_query", &words.join(" AND "))
                    .append_pair("start", "0")
                    .append_pair("max_results", &n);
            }
            _ => return Err("not a search source"),
        }
        Ok(url)
    }

    /// Headers beyond the User-Agent and `Accept-Encoding: gzip`.
    fn headers(self) -> &'static [(&'static str, &'static str)] {
        match self.service() {
            Service::GitHub => &[
                ("Accept", "application/vnd.github+json"),
                ("X-GitHub-Api-Version", "2022-11-28"),
            ],
            Service::Arxiv => &[("Accept", "application/atom+xml")],
            _ => &[("Accept", "application/json")],
        }
    }

    /// The results in a reply's body, at most `count`.
    /// Registries answer every query with their closest packages, however
    /// far; a package is kept only when it is about `query`
    /// ([`about_the_query`]).
    pub(crate) fn parse(
        self,
        endpoint: &Url,
        query: &str,
        body: &[u8],
        count: usize,
    ) -> Result<Vec<SearchResult>, String> {
        if self == Source::Arxiv {
            return Ok(parse_atom(
                &String::from_utf8_lossy(body),
                count,
                self.name(),
            ));
        }
        let v: Value = serde_json::from_slice(body)
            .map_err(|_| format!("the {} reply is not JSON", self.name()))?;
        let name = self.name();
        let hit = |title: String, url: String, snippet: String| SearchResult {
            title,
            url,
            snippet: cut(snippet),
            site_only: false,
            source: name,
        };
        let list = |key: &str| {
            v.get(key)
                .and_then(Value::as_array)
                .ok_or_else(|| format!("the {name} reply has no results list"))
        };
        let mut out = Vec::new();
        match self.service() {
            Service::Wikimedia => {
                let mut r = crate::search::Backend::Wikipedia {
                    endpoint: endpoint.clone(),
                }
                .parse(&v, count)?;
                r.iter_mut().for_each(|x| x.source = name);
                return Ok(r);
            }
            Service::StackExchange => {
                for q in list("items")? {
                    let (Some(title), Some(link)) = (str_of(q, "title"), str_of(q, "link")) else {
                        continue;
                    };
                    out.push(hit(clean(title), link.to_owned(), question_note(q)));
                }
            }
            Service::GitHub if self == Source::GitHub => {
                for r in list("items")? {
                    let (Some(title), Some(link)) = (str_of(r, "full_name"), str_of(r, "html_url"))
                    else {
                        continue;
                    };
                    out.push(hit(clean(title), link.to_owned(), repository_note(r)));
                }
            }
            Service::GitHub => {
                for r in list("items")? {
                    let (Some(title), Some(link)) = (str_of(r, "title"), str_of(r, "html_url"))
                    else {
                        continue;
                    };
                    out.push(hit(clean(title), link.to_owned(), issue_note(r)));
                }
            }
            Service::Crates => {
                for c in list("crates")? {
                    let Some(krate) = str_of(c, "name").filter(|_| about_the_query(c, query))
                    else {
                        continue;
                    };
                    out.push(hit(
                        krate.to_owned(),
                        format!("https://crates.io/crates/{krate}"),
                        crate_note(c, krate),
                    ));
                }
            }
            Service::Npm => {
                for o in list("objects")? {
                    let p = o.get("package").unwrap_or(&Value::Null);
                    let Some(package) = str_of(p, "name").filter(|_| about_the_query(p, query))
                    else {
                        continue;
                    };
                    let link = p.get("links").and_then(|l| str_of(l, "npm")).map_or_else(
                        || format!("https://www.npmjs.com/package/{package}"),
                        str::to_owned,
                    );
                    out.push(hit(
                        package.to_owned(),
                        link,
                        package_note(p, "description"),
                    ));
                }
            }
            Service::PyPi => {
                let info = v
                    .get("info")
                    .ok_or_else(|| format!("the {name} reply has no project"))?;
                if let Some(project) = str_of(info, "name") {
                    let link = str_of(info, "package_url").map_or_else(
                        || format!("https://pypi.org/project/{project}/"),
                        str::to_owned,
                    );
                    out.push(hit(project.to_owned(), link, package_note(info, "summary")));
                }
            }
            Service::HackerNews => {
                for h in list("hits")? {
                    let (Some(title), Some(id)) = (str_of(h, "title"), str_of(h, "objectID"))
                    else {
                        continue;
                    };
                    let discussion = format!("https://news.ycombinator.com/item?id={id}");
                    let link = str_of(h, "url")
                        .filter(|u| !u.is_empty())
                        .map_or_else(|| discussion.clone(), str::to_owned);
                    let note = format!(
                        "{} points, {} comments on Hacker News{}; discussion: {discussion}",
                        num(h, "points"),
                        num(h, "num_comments"),
                        str_of(h, "created_at")
                            .map(|d| format!(" ({})", d.chars().take(10).collect::<String>()))
                            .unwrap_or_default(),
                    );
                    out.push(hit(clean(title), link, note));
                }
            }
            Service::Arxiv => {}
        }
        out.truncate(count);
        Ok(out)
    }
}

/// Lower-case words of letters and digits.
fn words(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
}

/// Whether a package is about the query: at least half of the query's
/// words (of two or more characters) are words of its name, description or
/// keywords. `vite` (a build tool) for "vite build base path", `reqwest` (an
/// HTTP client) for "http client"; not `deno` for "tokio select".
fn about_the_query(package: &Value, query: &str) -> bool {
    let wanted: HashSet<String> = words(query).filter(|w| w.chars().count() > 1).collect();
    if wanted.is_empty() {
        return false;
    }
    let mut text: Vec<&str> = ["name", "description"]
        .iter()
        .filter_map(|k| str_of(package, k))
        .collect();
    if let Some(keywords) = package.get("keywords").and_then(Value::as_array) {
        text.extend(keywords.iter().filter_map(Value::as_str));
    }
    let have: HashSet<String> = text.iter().flat_map(|t| words(t)).collect();
    2 * wanted.iter().filter(|w| have.contains(*w)).count() >= wanted.len()
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn num(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// A Stack Exchange question's state: answers, score, tags.
fn question_note(q: &Value) -> String {
    let answers = num(q, "answer_count");
    let accepted = if q.get("accepted_answer_id").is_some() {
        ", one accepted"
    } else {
        ""
    };
    let tags: Vec<&str> = q
        .get("tags")
        .and_then(Value::as_array)
        .map(|t| t.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    format!(
        "{answers} answer{}{accepted}; score {}; tags: {}",
        if answers == 1 { "" } else { "s" },
        num(q, "score"),
        tags.join(", ")
    )
}

fn repository_note(r: &Value) -> String {
    let mut facts = vec![format!("{} stars", num(r, "stargazers_count"))];
    if let Some(lang) = str_of(r, "language") {
        facts.push(lang.to_owned());
    }
    if r.get("archived").and_then(Value::as_bool) == Some(true) {
        facts.push("archived".into());
    }
    let description = str_of(r, "description").map(clean).unwrap_or_default();
    format!("{description} ({})", facts.join(", "))
        .trim()
        .to_owned()
}

fn issue_note(r: &Value) -> String {
    let repo = str_of(r, "repository_url")
        .and_then(|u| u.split("/repos/").nth(1))
        .unwrap_or("?");
    let kind = if r.get("pull_request").is_some() {
        "pull request"
    } else {
        "issue"
    };
    let body: String = str_of(r, "body")
        .map(clean)
        .unwrap_or_default()
        .chars()
        .take(ISSUE_BODY_CHARS)
        .collect();
    format!(
        "{repo} {kind} #{}, {}, {} comments: {body}",
        num(r, "number"),
        str_of(r, "state").unwrap_or("?"),
        num(r, "comments"),
    )
}

fn crate_note(c: &Value, krate: &str) -> String {
    let version = ["max_stable_version", "default_version", "max_version"]
        .iter()
        .find_map(|k| str_of(c, k))
        .unwrap_or("?");
    let docs = str_of(c, "documentation")
        .map_or_else(|| format!("https://docs.rs/{krate}"), str::to_owned);
    format!(
        "{} (version {version}; {} downloads; docs: {docs})",
        str_of(c, "description").map(clean).unwrap_or_default(),
        num(c, "downloads"),
    )
}

fn package_note(p: &Value, key: &str) -> String {
    format!(
        "{} (version {})",
        str_of(p, key).map(clean).unwrap_or_default(),
        str_of(p, "version").unwrap_or("?"),
    )
}

/// A PyPI project name normalized (PEP 503), when the query is one.
fn pypi_name(query: &str) -> Option<String> {
    let q = query.trim();
    let ok = !q.is_empty()
        && q.len() <= 100
        && q.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && q.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        return None;
    }
    let mut out = String::new();
    for c in q.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !out.ends_with('-') {
                out.push('-');
            }
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    Some(out)
}

/// The text of the first `<tag ...>...</tag>` in `xml`.
fn element<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let mut from = 0;
    while let Some(at) = xml[from..].find(&open) {
        let start = from + at + open.len();
        // `<title>` or `<title attr=...>`, not `<titles>`.
        if xml[start..].starts_with(['>', ' ', '\n', '\t', '\r']) {
            let body = start + xml[start..].find('>')? + 1;
            let end = body + xml[body..].find(&format!("</{tag}>"))?;
            return Some(&xml[body..end]);
        }
        from = start;
    }
    None
}

/// The entries of an arXiv Atom feed.
fn parse_atom(xml: &str, count: usize, source: &'static str) -> Vec<SearchResult> {
    let mut out = Vec::new();
    for part in xml.split("<entry").skip(1) {
        let entry = part.split("</entry>").next().unwrap_or_default();
        let (Some(id), Some(title)) = (element(entry, "id"), element(entry, "title")) else {
            continue;
        };
        let id = id.trim();
        if !id.starts_with("http") {
            continue;
        }
        let url = id.replacen("http://", "https://", 1);
        let published = element(entry, "published")
            .map(|d| format!("{}: ", &d.trim()[..d.trim().len().min(10)]))
            .unwrap_or_default();
        let summary = element(entry, "summary").map(clean).unwrap_or_default();
        out.push(SearchResult {
            title: clean(title),
            url,
            snippet: cut(format!("{published}{summary}")),
            site_only: false,
            source,
        });
        if out.len() == count {
            break;
        }
    }
    out
}

/// One source of a run's native search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSetup {
    pub source: Source,
    pub endpoint: Url,
    /// Asked when the frontier names no sources.
    pub default: bool,
}

impl SourceSetup {
    pub fn new(source: Source, default: bool) -> Self {
        Self {
            source,
            endpoint: source.endpoint(),
            default,
        }
    }

    pub fn host(&self) -> String {
        self.endpoint.host_str().unwrap_or_default().to_owned()
    }
}

/// The native backend of a run: the sources it may ask, in rank order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSearch {
    pub sources: Vec<SourceSetup>,
}

impl NativeSearch {
    /// `defaults` asked by default, `on_request` only when named; both kept
    /// in rank order ([`ALL`]).
    pub fn new(defaults: &[Source], on_request: &[Source]) -> Self {
        let sources = ALL
            .iter()
            .filter(|s| defaults.contains(s) || on_request.contains(s))
            .map(|&s| SourceSetup::new(s, defaults.contains(&s)))
            .collect();
        Self { sources }
    }

    pub fn defaults(&self) -> impl Iterator<Item = &SourceSetup> {
        self.sources.iter().filter(|s| s.default)
    }

    pub fn on_request(&self) -> impl Iterator<Item = &SourceSetup> {
        self.sources.iter().filter(|s| !s.default)
    }

    /// The sources to ask: the named ones (each must be one of this run's),
    /// or the defaults.
    pub fn select(&self, names: Option<&[String]>) -> Result<Vec<&SourceSetup>, String> {
        let Some(names) = names else {
            return Ok(self.defaults().collect());
        };
        if names.is_empty() {
            return Err("name at least one source, or leave `sources` out".into());
        }
        let mut chosen = HashSet::new();
        for n in names {
            match self.sources.iter().find(|s| s.source.name() == n.trim()) {
                Some(s) => {
                    chosen.insert(s.source);
                }
                None => {
                    let known: Vec<&str> = self.sources.iter().map(|s| s.source.name()).collect();
                    return Err(format!(
                        "unknown source `{}`; this run's sources: {}",
                        n.trim(),
                        known.join(", ")
                    ));
                }
            }
        }
        Ok(self
            .sources
            .iter()
            .filter(|s| chosen.contains(&s.source))
            .collect())
    }

    /// The hosts that receive a query sent to `sources`, each once.
    pub fn hosts(sources: &[&SourceSetup]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in sources {
            let h = s.host();
            if !out.contains(&h) {
                out.push(h);
            }
        }
        out
    }
}

/// A service's pacing state, held for the length of one request so requests
/// to a service go one at a time.
#[derive(Debug, Default)]
struct Pacer {
    /// When the last request ended.
    last: Option<Instant>,
    /// When recent requests started (for a per-window budget).
    started: VecDeque<Instant>,
    /// The service asked us to wait until then.
    wait_until: Option<Instant>,
    /// `429`s in a row, for the doubling wait.
    strikes: u32,
}

impl Pacer {
    /// When the next request may start.
    fn ready_at(&mut self, now: Instant, service: Service) -> Instant {
        let mut ready = now;
        if let Some(last) = self.last {
            ready = ready.max(last + service.interval());
        }
        if let Some((max, per)) = service.window() {
            while self.started.front().is_some_and(|t| now >= *t + per) {
                self.started.pop_front();
            }
            if self.started.len() >= max
                && let Some(first) = self.started.front()
            {
                ready = ready.max(*first + per);
            }
        }
        ready
    }

    /// Waits `for_` from now (a `Retry-After`, a reset time) or, without one,
    /// a doubling wait.
    fn back_off(&mut self, now: Instant, for_: Option<Duration>) -> Duration {
        let wait = for_.unwrap_or_else(|| {
            FIRST_BACKOFF
                .saturating_mul(1 << self.strikes.min(8))
                .min(MAX_BACKOFF)
        });
        let wait = wait.min(MAX_BACKOFF.max(QUOTA_BACKOFF));
        self.strikes = self.strikes.saturating_add(1);
        self.wait_until = Some(self.wait_until.map_or(now + wait, |w| w.max(now + wait)));
        wait
    }
}

/// Per-run state of the native backend: pacing per service and answers.
#[derive(Default)]
pub(crate) struct NativeState {
    pacers: std::sync::Mutex<HashMap<Service, Arc<tokio::sync::Mutex<Pacer>>>>,
    cache: std::sync::Mutex<HashMap<(Source, String, usize), Vec<SearchResult>>>,
}

impl NativeState {
    fn pacer(&self, service: Service) -> Arc<tokio::sync::Mutex<Pacer>> {
        let mut map = self.pacers.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(service).or_default().clone()
    }

    fn cached(&self, key: &(Source, String, usize)) -> Option<Vec<SearchResult>> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.get(key).cloned()
    }

    fn keep(&self, key: (Source, String, usize), results: &[SearchResult]) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() < CACHE_ENTRIES {
            cache.insert(key, results.to_vec());
        }
    }
}

/// What a reply said about the service's limits.
#[derive(Debug, Default)]
struct Limits {
    retry_after: Option<Duration>,
    /// GitHub: requests left in this window, and when it resets.
    remaining: Option<u64>,
    reset_in: Option<Duration>,
}

fn limits(resp: &declass_net::Response) -> Limits {
    let get = |name: &str| resp.header(name).and_then(|v| v.trim().parse::<u64>().ok());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Limits {
        retry_after: get("retry-after").map(Duration::from_secs),
        remaining: get("x-ratelimit-remaining"),
        reset_in: get("x-ratelimit-reset")
            .map(|at| Duration::from_secs(at.saturating_sub(now) + 1)),
    }
}

/// Stack Exchange's own signals in a reply: `backoff` seconds, a spent
/// quota, a throttle violation (with the seconds it names).
fn stack_exchange_wait(body: &Value) -> Option<(Duration, &'static str)> {
    if body.get("error_name").and_then(Value::as_str) == Some("throttle_violation") {
        // "... more requests available in 82368 seconds": only the number.
        let secs = body
            .get("error_message")
            .and_then(Value::as_str)
            .and_then(|m| {
                m.split_whitespace()
                    .rev()
                    .find_map(|w| w.parse::<u64>().ok())
            })
            .unwrap_or(600);
        return Some((
            Duration::from_secs(secs).min(QUOTA_BACKOFF),
            "Stack Exchange's request limit for this address is reached",
        ));
    }
    if body.get("quota_remaining").and_then(Value::as_u64) == Some(0) {
        return Some((
            QUOTA_BACKOFF,
            "Stack Exchange's daily quota for this address is spent",
        ));
    }
    body.get("backoff")
        .and_then(Value::as_u64)
        .map(|s| (Duration::from_secs(s), "Stack Exchange asked to wait"))
}

/// Decodes a gzip body (Stack Exchange compresses every reply; the others
/// may when asked), reading at most `cap` bytes of it.
pub(crate) fn decompress(bytes: Vec<u8>, encoding: &str, cap: usize) -> Result<Vec<u8>, WebError> {
    let encoding = encoding.trim().to_ascii_lowercase();
    let gzip = matches!(encoding.as_str(), "gzip" | "x-gzip")
        || (encoding.is_empty() && bytes.starts_with(&[0x1f, 0x8b]));
    if !gzip {
        return match encoding.as_str() {
            "" | "identity" => Ok(bytes),
            other => Err(WebError::Search(format!(
                "a reply in an encoding that is not read ({other})"
            ))),
        };
    }
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(&bytes[..])
        .take(cap as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|_| WebError::Search("a compressed reply that does not decompress".into()))?;
    if out.len() > cap {
        return Err(WebError::Search(
            "the reply is larger than web.max_bytes once decompressed".into(),
        ));
    }
    Ok(out)
}

/// The key two results with the same address share: no scheme, no `www.`,
/// no trailing slash, no fragment.
fn same_page(url: &str) -> String {
    match Url::parse(url) {
        Ok(u) => {
            let host = u.host_str().unwrap_or_default().to_ascii_lowercase();
            let host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
            let path = u.path().trim_end_matches('/');
            match u.query() {
                Some(q) => format!("{host}{path}?{q}"),
                None => format!("{host}{path}"),
            }
        }
        Err(_) => url.trim().to_ascii_lowercase(),
    }
}

/// Merges per-source lists (in rank order): every source's first result,
/// then every source's second, and so on; a page already listed is left out.
pub fn merge(lists: &[Vec<SearchResult>], count: usize) -> Vec<SearchResult> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let longest = lists.iter().map(Vec::len).max().unwrap_or(0);
    for rank in 0..longest {
        for r in lists.iter().filter_map(|l| l.get(rank)) {
            if out.len() == count {
                return out;
            }
            if seen.insert(same_page(&r.url)) {
                out.push(r.clone());
            }
        }
    }
    out
}

/// What one source did for one search.
struct Asked {
    report: SourceReport,
    results: Vec<SearchResult>,
}

impl Web {
    /// Asks the chosen sources in parallel, each within its own time, and
    /// merges what they answer. Every source gets a report (for the audit
    /// log and the frontier), whether it answered, was skipped or failed.
    pub(crate) async fn native_search(
        &self,
        guard: &Guard,
        native: &NativeSearch,
        query: &str,
        count: usize,
        names: Option<&[String]>,
    ) -> Result<Searched, WebError> {
        let chosen = native.select(names).map_err(WebError::Invalid)?;
        let budget = SOURCE_TIMEOUT.min(self.cfg.timeout);
        let asked = futures_util::future::join_all(
            chosen
                .iter()
                .map(|s| self.ask(guard, s, query, count, budget)),
        )
        .await;
        let lists: Vec<Vec<SearchResult>> = asked.iter().map(|a| a.results.clone()).collect();
        Ok(Searched {
            results: merge(&lists, count),
            requests: asked.into_iter().map(|a| a.report).collect(),
            labelled: true,
        })
    }

    async fn ask(
        &self,
        guard: &Guard,
        setup: &SourceSetup,
        query: &str,
        count: usize,
        budget: Duration,
    ) -> Asked {
        let source = setup.source;
        let report = |bytes: usize, outcome: &'static str, note: String| SourceReport {
            source: source.name(),
            host: setup.host(),
            bytes,
            outcome,
            note,
            hits: 0,
        };
        let url = match source.request(&setup.endpoint, query, count) {
            Ok(u) => u,
            Err(why) => {
                return Asked {
                    report: report(0, "not_applicable", why.into()),
                    results: Vec::new(),
                };
            }
        };
        // Checked before its turn is waited for: a refused request takes none.
        let request = match self.native_request(guard, setup, url) {
            Ok(r) => r,
            Err(e) => {
                return Asked {
                    report: report(0, e.outcome(), e.to_string()),
                    results: Vec::new(),
                };
            }
        };
        let key = (
            source,
            query.split_whitespace().collect::<Vec<_>>().join(" "),
            count,
        );
        if let Some(results) = self.native.cached(&key) {
            return Asked {
                report: SourceReport {
                    hits: results.len(),
                    ..report(0, "cached", "answered earlier in this run".into())
                },
                results,
            };
        }
        let deadline = Instant::now() + budget;
        let asked = tokio::time::timeout_at(
            deadline,
            self.ask_now(setup, &request, query, count, budget, deadline),
        )
        .await;
        match asked {
            Ok(Ok((bytes, results))) => {
                self.native.keep(key, &results);
                // These match every word; a frontier that asked a sentence
                // learns why nothing came back.
                let every_word =
                    matches!(source.service(), Service::StackExchange | Service::GitHub);
                let note =
                    if results.is_empty() && every_word && query.split_whitespace().count() > 2 {
                        "it matches every word: try fewer".to_owned()
                    } else {
                        String::new()
                    };
                Asked {
                    report: SourceReport {
                        hits: results.len(),
                        ..report(bytes, "ok", note)
                    },
                    results,
                }
            }
            Ok(Err((bytes, outcome, note))) => Asked {
                report: report(bytes, outcome, note),
                results: Vec::new(),
            },
            Err(_) => Asked {
                report: report(
                    0,
                    "timeout",
                    WebError::Timeout(budget.as_secs()).to_string(),
                ),
                results: Vec::new(),
            },
        }
    }

    /// Waits for the service's turn (or gives up when it is too far off),
    /// makes the request and learns the service's limits from the reply.
    async fn ask_now(
        &self,
        setup: &SourceSetup,
        request: &Checked,
        query: &str,
        count: usize,
        budget: Duration,
        deadline: Instant,
    ) -> Result<(usize, Vec<SearchResult>), (usize, &'static str, String)> {
        let source = setup.source;
        let service = source.service();
        let pacer = self.native.pacer(service);
        let mut pace = pacer.lock().await;
        let now = Instant::now();
        if let Some(until) = pace.wait_until.filter(|u| *u > now) {
            return Err((
                0,
                "backoff",
                format!(
                    "the service asked for a pause; about {} s left",
                    (until - now).as_secs() + 1
                ),
            ));
        }
        let ready = pace.ready_at(now, service);
        if ready > now + budget / 2 {
            let limit = match service.window() {
                Some((n, per)) => format!("{n} requests in {} s without an account", per.as_secs()),
                None => format!("one request every {} s", service.interval().as_secs()),
            };
            return Err((
                0,
                "throttled",
                format!(
                    "its rate limit ({limit}) is reached; about {} s until the next request",
                    (ready - now).as_secs() + 1
                ),
            ));
        }
        tokio::time::sleep_until(ready).await;
        pace.started.push_back(Instant::now());
        let reply = self.native_get(setup, request, deadline).await;
        let now = Instant::now();
        pace.last = Some(now);
        let (status, limits, body) = match reply {
            Ok(r) => r,
            // The source's own time, not the fetch's.
            Err(WebError::Timeout(_)) => {
                let e = WebError::Timeout(budget.as_secs());
                return Err((0, e.outcome(), e.to_string()));
            }
            Err(e) => return Err((0, e.outcome(), e.to_string())),
        };
        let bytes = body.len();
        let name = source.name();
        // Limits first: they hold whether or not this request succeeded.
        let json: Option<Value> = (service == Service::StackExchange)
            .then(|| serde_json::from_slice(&body).ok())
            .flatten();
        let exhausted = service == Service::GitHub && limits.remaining == Some(0);
        // GitHub answers its secondary limits with a 403 and `Retry-After`.
        if status == 429 || (status == 403 && (exhausted || limits.retry_after.is_some())) {
            let waited = pace.back_off(now, limits.retry_after.or(limits.reset_in));
            return Err((
                bytes,
                "rate_limited",
                format!(
                    "{name} answered HTTP {status} (rate limit); not asked again for {} s",
                    waited.as_secs()
                ),
            ));
        }
        if exhausted {
            pace.back_off(now, limits.reset_in);
        }
        if let Some((wait, why)) = json.as_ref().and_then(stack_exchange_wait) {
            pace.back_off(now, Some(wait));
            if !(200..300).contains(&status) {
                return Err((
                    bytes,
                    "rate_limited",
                    format!("{why}; not asked again for {} s", wait.as_secs()),
                ));
            }
        }
        if (200..300).contains(&status) {
            pace.strikes = 0;
        }
        drop(pace);
        if source == Source::PyPi && status == 404 {
            return Ok((bytes, Vec::new()));
        }
        if !(200..300).contains(&status) {
            // The reply's text is not shown: it would reach the frontier
            // outside the presenter.
            return Err((
                bytes,
                "search_error",
                format!("{name} answered HTTP {status}"),
            ));
        }
        source
            .parse(&setup.endpoint, query, &body, count)
            .map(|r| (bytes, r))
            .map_err(|e| (bytes, "search_error", e))
    }

    /// A source's `GET`, checked in every part by `guard` (the query is in
    /// its URL).
    fn native_request(
        &self,
        guard: &Guard,
        setup: &SourceSetup,
        url: Url,
    ) -> Result<Checked, WebError> {
        Self::check_form(&url)?;
        let mut out = Outgoing::get(url).header("Accept-Encoding", "gzip");
        for (k, v) in setup.source.headers() {
            out = out.header(*k, *v);
        }
        Self::checked(guard, "web_search", out)
    }

    /// One guarded `GET` of a source's API: the address checked and pinned
    /// like a fetch, no redirects, the body capped and decompressed.
    async fn native_get(
        &self,
        setup: &SourceSetup,
        request: &Checked,
        deadline: Instant,
    ) -> Result<(u16, Limits, Vec<u8>), WebError> {
        let pinned = self.checked_addr(request, deadline).await?;
        let host = request.url().host_str().unwrap_or_default().to_owned();
        let client = self.client(
            pinned.map(|a| (host.as_str(), a)),
            self.remaining(deadline)?,
        )?;
        let mut resp = client
            .send(request, &Credentials::default())
            .await
            .map_err(|e| self.map_err(e))?;
        let status = resp.status();
        let limits = limits(&resp);
        let encoding = resp
            .header("content-encoding")
            .unwrap_or_default()
            .to_owned();
        let (bytes, truncated) = self.read_capped(&mut resp, deadline).await?;
        if truncated {
            return Err(WebError::Search(format!(
                "the {} reply is larger than web.max_bytes",
                setup.source.name()
            )));
        }
        let body = decompress(bytes, &encoding, self.cfg.max_bytes)?;
        Ok((status, limits, body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ep(s: Source) -> Url {
        s.endpoint()
    }

    fn parse(s: Source, v: Value) -> Vec<SearchResult> {
        s.parse(&ep(s), "tokio vite", v.to_string().as_bytes(), 10)
            .unwrap()
    }

    fn pairs(url: &Url) -> Vec<(String, String)> {
        url.query_pairs().into_owned().collect()
    }

    #[test]
    fn names_round_trip_and_every_source_has_an_https_endpoint() {
        for &s in ALL {
            assert_eq!(Source::from_name(s.name()), Some(s));
            assert_eq!(s.endpoint().scheme(), "https", "{s:?}");
            assert!(!s.about().is_empty());
        }
        assert_eq!(Source::from_name("google"), None);
        let mut sorted = ALL.to_vec();
        sorted.sort();
        assert_eq!(sorted, ALL, "ALL is in rank order");
    }

    #[test]
    fn requests_carry_the_query_and_count_each_source_expects() {
        let q = "tokio select";
        let u = Source::StackOverflow
            .request(&ep(Source::StackOverflow), q, 4)
            .unwrap();
        assert_eq!(u.host_str(), Some("api.stackexchange.com"));
        let p = pairs(&u);
        for want in [
            ("q", q),
            ("site", "stackoverflow"),
            ("pagesize", "4"),
            ("sort", "relevance"),
        ] {
            assert!(p.contains(&(want.0.into(), want.1.into())), "{p:?}");
        }
        let u = Source::Unix.request(&ep(Source::Unix), q, 4).unwrap();
        assert!(pairs(&u).contains(&("site".into(), "unix".into())));
        let u = Source::GitHubIssues
            .request(&ep(Source::GitHubIssues), q, 3)
            .unwrap();
        assert_eq!(u.path(), "/search/issues");
        assert!(pairs(&u).contains(&("per_page".into(), "3".into())));
        assert!(
            Source::GitHub
                .request(&ep(Source::GitHub), &"x ".repeat(200), 3)
                .is_err()
        );
        let u = Source::Crates.request(&ep(Source::Crates), q, 5).unwrap();
        assert_eq!(
            u.as_str(),
            "https://crates.io/api/v1/crates?q=tokio+select&per_page=5"
        );
        let u = Source::Npm.request(&ep(Source::Npm), q, 5).unwrap();
        assert_eq!(
            u.as_str(),
            "https://registry.npmjs.org/-/v1/search?text=tokio+select&size=5"
        );
        let u = Source::HackerNews
            .request(&ep(Source::HackerNews), q, 5)
            .unwrap();
        assert!(pairs(&u).contains(&("tags".into(), "story".into())));
        let u = Source::Arxiv
            .request(&ep(Source::Arxiv), "async/await: cancellation!", 2)
            .unwrap();
        assert!(
            pairs(&u).contains(&(
                "search_query".into(),
                "all:async AND all:await AND all:cancellation".into()
            )),
            "{u}"
        );
        assert!(Source::Arxiv.request(&ep(Source::Arxiv), "?!", 2).is_err());
        let u = Source::Wikipedia
            .request(&ep(Source::Wikipedia), q, 2)
            .unwrap();
        assert!(pairs(&u).contains(&("srsearch".into(), q.into())));
    }

    #[test]
    fn pypi_is_asked_for_exact_names_only() {
        let u = Source::PyPi
            .request(&ep(Source::PyPi), "Flask_SQLAlchemy", 5)
            .unwrap();
        assert_eq!(u.as_str(), "https://pypi.org/pypi/flask-sqlalchemy/json");
        for q in ["vite build base path", "", "../admin", "-x", "a/b"] {
            assert!(
                Source::PyPi.request(&ep(Source::PyPi), q, 5).is_err(),
                "{q}"
            );
        }
        assert_eq!(
            pypi_name("zope.interface").as_deref(),
            Some("zope-interface")
        );
    }

    #[test]
    fn stack_exchange_questions_show_answers_score_and_tags() {
        // The shape of a live reply (2026-09-26), shortened.
        let r = parse(
            Source::StackOverflow,
            json!({"items": [
                {"tags": ["rust", "rust-tokio"], "is_answered": true, "accepted_answer_id": 79479592,
                 "answer_count": 1, "score": 1, "question_id": 79479269,
                 "link": "https://stackoverflow.com/questions/79479269/how-does-rust-handle",
                 "title": "How does Rust handle cleanup when tokio::select! cancels a future?"},
                {"tags": [], "answer_count": 0, "score": -1, "link": "https://stackoverflow.com/q/1",
                 "title": "Vite &#39;base&#39; path"},
                {"title": "no link"}
            ], "has_more": true, "quota_max": 300, "quota_remaining": 294}),
        );
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].source, "stackoverflow");
        assert_eq!(
            r[0].snippet,
            "1 answer, one accepted; score 1; tags: rust, rust-tokio"
        );
        assert_eq!(r[1].title, "Vite 'base' path");
        assert!(r[1].snippet.starts_with("0 answers; score -1"));
        assert!(
            Source::StackOverflow
                .parse(&ep(Source::StackOverflow), "q", b"{}", 5)
                .is_err()
        );
    }

    #[test]
    fn stack_exchange_limits_are_read_from_the_reply() {
        let w = stack_exchange_wait(&json!({"items": [], "backoff": 10, "quota_remaining": 12}))
            .unwrap();
        assert_eq!(w.0, Duration::from_secs(10));
        let w = stack_exchange_wait(&json!({"items": [], "quota_remaining": 0})).unwrap();
        assert_eq!(w.0, QUOTA_BACKOFF);
        let w = stack_exchange_wait(&json!({"error_id": 502, "error_name": "throttle_violation",
            "error_message": "too many requests from this IP, more requests available in 120 seconds"}))
        .unwrap();
        assert_eq!(w.0, Duration::from_secs(120));
        assert!(stack_exchange_wait(&json!({"items": [], "quota_remaining": 250})).is_none());
    }

    #[test]
    fn github_repositories_and_issues_parse() {
        let r = parse(
            Source::GitHub,
            json!({"total_count": 1, "items": [{"full_name": "tokio-rs/tokio",
                "html_url": "https://github.com/tokio-rs/tokio", "description": "A runtime",
                "stargazers_count": 31000, "language": "Rust", "archived": false}]}),
        );
        assert_eq!(r[0].title, "tokio-rs/tokio");
        assert_eq!(r[0].snippet, "A runtime (31000 stars, Rust)");
        let r = parse(
            Source::GitHubIssues,
            json!({"total_count": 2, "items": [
                {"title": "Document cancellation safety", "html_url": "https://github.com/a/b/issues/7",
                 "repository_url": "https://api.github.com/repos/a/b", "number": 7, "state": "open",
                 "comments": 3, "body": "The **select** docs"},
                {"title": "Fix", "html_url": "https://github.com/a/b/pull/8", "number": 8,
                 "repository_url": "https://api.github.com/repos/a/b", "state": "closed",
                 "comments": 0, "pull_request": {}, "body": null}]}),
        );
        assert_eq!(
            r[0].snippet,
            "a/b issue #7, open, 3 comments: The **select** docs"
        );
        assert!(r[1].snippet.starts_with("a/b pull request #8, closed"));
    }

    #[test]
    fn registries_parse_with_package_pages() {
        let r = parse(
            Source::Crates,
            json!({"crates": [{"name": "tokio-context", "description": "Contexts for cancelling",
                "max_stable_version": "0.1.3", "downloads": 85336, "documentation": null}], "meta": {}}),
        );
        assert_eq!(r[0].url, "https://crates.io/crates/tokio-context");
        assert_eq!(
            r[0].snippet,
            "Contexts for cancelling (version 0.1.3; 85336 downloads; docs: https://docs.rs/tokio-context)"
        );
        let r = parse(
            Source::Npm,
            json!({"objects": [{"package": {"name": "vite", "version": "8.3.1",
                "description": "Native-ESM powered web dev build tool",
                "links": {"npm": "https://www.npmjs.com/package/vite"}}}], "total": 1}),
        );
        assert_eq!(r[0].url, "https://www.npmjs.com/package/vite");
        assert_eq!(
            r[0].snippet,
            "Native-ESM powered web dev build tool (version 8.3.1)"
        );
        let r = parse(
            Source::PyPi,
            json!({"info": {"name": "requests", "version": "2.34.2", "summary": "Python HTTP for Humans.",
                "package_url": "https://pypi.org/project/requests/"}}),
        );
        assert_eq!(r[0].title, "requests");
        assert_eq!(r[0].snippet, "Python HTTP for Humans. (version 2.34.2)");
    }

    #[test]
    fn registry_packages_must_be_about_the_query() {
        // Live npm answers (2026-09-26) to "vite build base path", shortened.
        let body = json!({"objects": [
            {"package": {"name": "vite", "version": "8.3.1",
                "description": "Native-ESM powered web dev build tool"}},
            {"package": {"name": "vite-static", "version": "1",
                "description": "Embed Vite chunks into your Rust application"}},
            {"package": {"name": "base", "version": "3",
                "description": "Framework for rapidly creating high quality node.js applications",
                "keywords": ["path", "build"]}},
            {"package": {"name": "deno", "version": "2", "description": "A runtime"}}
        ]});
        let r = Source::Npm
            .parse(
                &ep(Source::Npm),
                "vite build base path",
                body.to_string().as_bytes(),
                10,
            )
            .unwrap();
        let names: Vec<&str> = r.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(names, ["vite", "base"]);
        let about = |v: Value, q: &str| about_the_query(&v, q);
        assert!(about(
            json!({"name": "reqwest", "description": "higher level HTTP client library"}),
            "http client"
        ));
        assert!(about(json!({"name": "tokio_util"}), "Tokio select"));
        assert!(!about(
            json!({"name": "wiremock-captain", "description": "test your HTTP APIs"}),
            "Captain Comic 1988 DOS game"
        ));
        assert!(
            !about(json!({"name": "a-b"}), "a b"),
            "one-letter words match nothing"
        );
    }

    #[test]
    fn hacker_news_links_the_story_and_its_discussion() {
        let r = parse(
            Source::HackerNews,
            json!({"hits": [
                {"title": "Async cancellation", "url": "https://example.org/post", "objectID": "42",
                 "points": 120, "num_comments": 30, "created_at": "2024-03-01T10:00:00Z"},
                {"title": "Ask HN: tokio?", "url": null, "objectID": "43", "points": 5, "num_comments": 2}
            ]}),
        );
        assert_eq!(r[0].url, "https://example.org/post");
        assert!(
            r[0].snippet
                .contains("120 points, 30 comments on Hacker News (2024-03-01)")
        );
        assert!(
            r[0].snippet
                .ends_with("https://news.ycombinator.com/item?id=42")
        );
        assert_eq!(r[1].url, "https://news.ycombinator.com/item?id=43");
    }

    #[test]
    fn arxiv_entries_parse_from_atom() {
        let feed = r#"<?xml version='1.0' encoding='UTF-8'?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>arXiv Query</title>
  <entry>
    <id>http://arxiv.org/abs/2608.20677v1</id>
    <title>A Design Space Exploration of
      Async/Await</title>
    <published>2026-08-21T02:22:38Z</published>
    <summary>Many languages &amp; runtimes.</summary>
  </entry>
  <entry><id>no-link</id><title>x</title></entry>
</feed>"#;
        let r = Source::Arxiv
            .parse(&ep(Source::Arxiv), "q", feed.as_bytes(), 5)
            .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].url, "https://arxiv.org/abs/2608.20677v1");
        assert_eq!(r[0].title, "A Design Space Exploration of Async/Await");
        assert_eq!(r[0].snippet, "2026-08-21: Many languages & runtimes.");
        assert!(
            Source::Arxiv
                .parse(&ep(Source::Arxiv), "q", b"<feed/>", 5)
                .unwrap()
                .is_empty()
        );
    }

    fn hit(source: &'static str, url: &str) -> SearchResult {
        SearchResult {
            title: url.into(),
            url: url.into(),
            snippet: String::new(),
            site_only: false,
            source,
        }
    }

    #[test]
    fn merging_interleaves_by_rank_and_drops_repeated_pages() {
        let lists = vec![
            vec![
                hit("stackoverflow", "https://stackoverflow.com/q/1"),
                hit("stackoverflow", "https://stackoverflow.com/q/2"),
            ],
            vec![],
            vec![
                hit("github", "https://github.com/a/b"),
                hit("github", "http://www.stackoverflow.com/q/1/"),
                hit("github", "https://github.com/c/d"),
            ],
            vec![hit("wikipedia", "https://en.wikipedia.org/wiki/A")],
        ];
        let urls: Vec<String> = merge(&lists, 10).into_iter().map(|r| r.url).collect();
        assert_eq!(
            urls,
            [
                "https://stackoverflow.com/q/1",
                "https://github.com/a/b",
                "https://en.wikipedia.org/wiki/A",
                "https://stackoverflow.com/q/2",
                "https://github.com/c/d",
            ]
        );
        assert_eq!(merge(&lists, 2).len(), 2);
        assert!(merge(&[], 5).is_empty());
    }

    #[test]
    fn selection_keeps_rank_order_and_refuses_unknown_sources() {
        let n = NativeSearch::new(
            &[Source::Wikipedia, Source::StackOverflow, Source::GitHub],
            &[Source::Arxiv, Source::Npm],
        );
        let names = |v: Vec<&SourceSetup>| v.iter().map(|s| s.source.name()).collect::<Vec<_>>();
        assert_eq!(
            names(n.select(None).unwrap()),
            ["stackoverflow", "wikipedia", "github"]
        );
        let pick = vec!["arxiv".to_owned(), "wikipedia".to_owned()];
        assert_eq!(
            names(n.select(Some(&pick)).unwrap()),
            ["wikipedia", "arxiv"]
        );
        let e = n.select(Some(&["google".to_owned()])).unwrap_err();
        assert!(
            e.contains("unknown source `google`") && e.contains("npm"),
            "{e}"
        );
        assert!(n.select(Some(&["crates".to_owned()])).is_err());
        assert!(n.select(Some(&[])).is_err());
        assert_eq!(
            NativeSearch::hosts(&n.select(None).unwrap()),
            [
                "api.stackexchange.com",
                "en.wikipedia.org",
                "api.github.com"
            ]
        );
    }

    #[test]
    fn pacing_spaces_requests_and_counts_the_github_window() {
        let t0 = Instant::now();
        let mut p = Pacer::default();
        assert_eq!(p.ready_at(t0, Service::Crates), t0);
        p.last = Some(t0);
        assert_eq!(p.ready_at(t0, Service::Crates), t0 + Duration::from_secs(1));
        assert_eq!(p.ready_at(t0, Service::Arxiv), t0 + Duration::from_secs(3));
        let mut g = Pacer::default();
        for i in 0..10 {
            g.started.push_back(t0 + Duration::from_millis(i));
        }
        assert_eq!(
            g.ready_at(t0 + Duration::from_secs(1), Service::GitHub),
            t0 + Duration::from_secs(60)
        );
        assert_eq!(
            g.ready_at(t0 + Duration::from_secs(61), Service::GitHub),
            t0 + Duration::from_secs(61)
        );
        // A 429 without Retry-After waits 30 s, then 60 s.
        let mut b = Pacer::default();
        assert_eq!(b.back_off(t0, None), Duration::from_secs(30));
        assert_eq!(b.back_off(t0, None), Duration::from_secs(60));
        assert_eq!(
            b.back_off(t0, Some(Duration::from_secs(5))),
            Duration::from_secs(5)
        );
        assert_eq!(b.wait_until, Some(t0 + Duration::from_secs(60)));
    }

    #[test]
    fn gzip_replies_are_decompressed_within_the_cap() {
        use flate2::write::GzEncoder;
        use std::io::Write;
        let mut e = GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(br#"{"items":[]}"#).unwrap();
        let gz = e.finish().unwrap();
        assert_eq!(
            decompress(gz.clone(), "gzip", 1000).unwrap(),
            br#"{"items":[]}"#
        );
        // Stack Exchange compresses even when the header is lost on the way.
        assert_eq!(
            decompress(gz.clone(), "", 1000).unwrap(),
            br#"{"items":[]}"#
        );
        assert!(decompress(gz, "gzip", 5).is_err());
        assert_eq!(decompress(b"{}".to_vec(), "", 10).unwrap(), b"{}");
        assert!(decompress(b"{}".to_vec(), "br", 10).is_err());
        assert!(decompress(b"not gzip".to_vec(), "gzip", 10).is_err());
    }
}
