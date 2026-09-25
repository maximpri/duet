// SPDX-License-Identifier: GPL-3.0-or-later
//! Search backends: a self-hosted SearXNG instance (JSON API, no key) or the
//! Brave Search API (key from an environment variable).

use serde_json::Value;
use url::Url;

/// The Brave Search web endpoint.
pub const BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

/// One search hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Where searches go. The key is held only in memory and never printed.
#[derive(Clone)]
pub enum Backend {
    Searxng { base: Url },
    Brave { endpoint: Url, key: String },
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Backend::Searxng { base } => write!(f, "Searxng({base})"),
            Backend::Brave { endpoint, .. } => write!(f, "Brave({endpoint}, key withheld)"),
        }
    }
}

impl Backend {
    pub fn name(&self) -> &'static str {
        match self {
            Backend::Searxng { .. } => "searxng",
            Backend::Brave { .. } => "brave",
        }
    }

    /// The host queries are sent to (for outbound checks and the audit log).
    pub fn host(&self) -> String {
        let url = match self {
            Backend::Searxng { base } => base,
            Backend::Brave { endpoint, .. } => endpoint,
        };
        url.host_str().unwrap_or_default().to_owned()
    }

    /// The request URL and extra headers for `query`.
    pub(crate) fn request(&self, query: &str, count: usize) -> (Url, Vec<(&'static str, String)>) {
        match self {
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
                (url, Vec::new())
            }
            Backend::Brave { endpoint, key } => {
                let mut url = endpoint.clone();
                url.query_pairs_mut()
                    .append_pair("q", query)
                    .append_pair("count", &count.to_string());
                (url, vec![("X-Subscription-Token", key.clone())])
            }
        }
    }

    /// Parses a backend's JSON reply into at most `count` results.
    pub fn parse(&self, body: &Value, count: usize) -> Result<Vec<SearchResult>, String> {
        let (list, snippet_key) = match self {
            Backend::Searxng { .. } => (body.get("results"), "content"),
            Backend::Brave { .. } => (
                body.get("web").and_then(|w| w.get("results")),
                "description",
            ),
        };
        let Some(list) = list else {
            // Brave answers a query without web hits with no `web` section.
            if matches!(self, Backend::Brave { .. }) && body.get("type").is_some() {
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
        Ok(list
            .iter()
            .filter_map(|r| {
                let url = r.get("url").and_then(Value::as_str)?;
                Some(SearchResult {
                    title: text(r, "title"),
                    url: url.to_owned(),
                    snippet: text(r, snippet_key),
                })
            })
            .take(count)
            .collect())
    }
}

/// Snippets carry highlighting markup (`<strong>`) and entities.
fn clean(s: &str) -> String {
    crate::html::to_text(s, None)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Search results as the frontier reads them.
pub fn render(query: &str, results: &[SearchResult]) -> String {
    if results.is_empty() {
        return format!("No results for {query:?}.\n");
    }
    let mut out = format!("Results for {query:?}:\n");
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!("{}. {}\n   {}\n", i + 1, r.title, r.url));
        if !r.snippet.is_empty() {
            out.push_str(&format!("   {}\n", r.snippet));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn both_backends_parse_title_url_and_snippet() {
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
        assert_eq!(searx.parse(&body, 1).unwrap().len(), 1);
        let (url, headers) = searx.request("a b", 5);
        assert_eq!(
            url.as_str(),
            "http://127.0.0.1:8888/search?q=a+b&format=json"
        );
        assert!(headers.is_empty());

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
}
