// SPDX-License-Identifier: GPL-3.0-or-later
//! The run's web access, from the `web.*` settings, and the choice of search
//! backend (`web.search.backend`, `auto` by default). `auto` is Z.ai's search
//! when the frontier is Z.ai and its key is set (queries go only to the
//! provider that already receives the run), else the native backend: the host
//! asks public sources with open APIs itself (Stack Overflow, Wikipedia,
//! GitHub and the workspace's package registries by default;
//! `web.search.sources`), with no search provider in between. The owner's
//! SearXNG, Brave and Wikipedia alone are used only when the owner selects
//! them by name: a SearXNG URL or a Brave key alone does not select them.

use anyhow::{Context, Result, bail};
use declass_config::Config;
use declass_web::guard::Allowlist;
use declass_web::search::native::{ALL, NativeSearch, Source, SourceSetup};
use declass_web::search::{
    BRAVE_ENDPOINT, Backend, WIKIPEDIA_ENDPOINT, ZAI_ENDPOINT, ZAI_ENGINES, ZAI_PLAN_ENDPOINT,
};
use declass_web::{Web, WebConfig};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// The key variable an explicit `zai` backend uses when the frontier is not
/// Z.ai (another provider's key is never sent to Z.ai).
pub const ZAI_KEY_ENV: &str = "ZAI_API_KEY";

/// The frontier a run talks to: the `zai` backend searches with its key only
/// when it is Z.ai.
#[derive(Debug, Clone, Copy)]
pub struct Frontier<'a> {
    pub base_url: &'a str,
    pub key_env: &'a str,
}

impl Frontier<'_> {
    fn host_is_zai(&self) -> bool {
        url::Url::parse(self.base_url).is_ok_and(|u| {
            u.host_str()
                .is_some_and(|h| h.eq_ignore_ascii_case("api.z.ai"))
        })
    }

    /// The GLM Coding Plan's endpoint (`https://api.z.ai/api/coding/...`).
    fn coding_plan(&self) -> bool {
        self.host_is_zai()
            && url::Url::parse(self.base_url).is_ok_and(|u| u.path().starts_with("/api/coding/"))
    }
}

/// The search backend a run gets, and why.
#[derive(Debug)]
pub struct Choice {
    pub backend: Option<Backend>,
    /// One line: which backend searches and who receives the queries, or why
    /// there is none.
    pub detail: String,
    /// The owner chose a backend that cannot be used (web_search is off).
    pub problem: Option<String>,
}

fn chosen(backend: Backend, detail: String) -> Choice {
    Choice {
        backend: Some(backend),
        detail,
        problem: None,
    }
}

fn off(problem: String) -> Choice {
    Choice {
        backend: None,
        detail: format!("{problem}; web_search is off"),
        problem: Some(problem),
    }
}

/// A variable's value when it is set and not blank.
fn var(env: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    env(name)
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// Web access for this run, or `None` when `web.enabled` is off. `frontier`
/// is the run's frontier; `ws` the workspace,
/// whose languages pick the native backend's package registries. A search
/// backend that cannot be used leaves `web_search` out with a warning;
/// `web_fetch` is still offered.
pub fn access(cfg: &Config, frontier: Option<Frontier<'_>>, ws: &Path) -> Result<Option<Arc<Web>>> {
    if !cfg.bool("web.enabled")? {
        return Ok(None);
    }
    let allowlist = Allowlist::parse(&cfg.list("web.allowlist_private")?)
        .map_err(anyhow::Error::msg)
        .context("web.allowlist_private")?;
    let choice = choose(cfg, frontier, &|name| std::env::var(name).ok(), ws)?;
    match &choice.problem {
        Some(p) => eprintln!("warning: {p}; web_search is off"),
        None if choice.backend.is_some() => eprintln!("web search: {}", choice.detail),
        None => {}
    }
    Ok(Some(Arc::new(Web::new(WebConfig {
        max_bytes: cfg.int("web.max_bytes")? as usize,
        timeout: Duration::from_secs(cfg.int("web.timeout_secs")? as u64),
        allowlist,
        search: choice.backend,
    }))))
}

/// The search backend from the settings, the run's frontier, the
/// environment (`env` looks variables up; keys are read, never printed) and
/// the workspace (`ws`: its languages).
pub fn choose(
    cfg: &Config,
    frontier: Option<Frontier<'_>>,
    env: &dyn Fn(&str) -> Option<String>,
    ws: &Path,
) -> Result<Choice> {
    let setting = cfg.str("web.search.backend")?;
    Ok(match setting.as_str() {
        "none" => Choice {
            backend: None,
            detail: "web.search.backend is none: web_search is not offered".into(),
            problem: None,
        },
        "zai" => match zai(cfg, frontier, env)? {
            Ok(c) => c,
            Err(why) => off(format!("web.search.backend is zai but {why}")),
        },
        "searxng" => match searxng(cfg)? {
            Some(c) => c,
            None => off("web.search.backend is searxng but web.search.searxng_url is empty".into()),
        },
        "brave" => match brave(cfg, env)? {
            Ok(c) => c,
            Err(why) => off(format!("web.search.backend is brave but {why}")),
        },
        "wikipedia" => wikipedia("web.search.backend is wikipedia"),
        "native" => native(cfg, ws, None)?,
        _ => {
            // `auto` with a Z.ai frontier and its key: Z.ai's search, whose
            // queries go only to the provider that already receives the run
            // (the native sources found little for open-ended research: the
            // same task reached MobyGames with it and Wikipedia alone
            // without). Anything else: the native backend, and what an
            // earlier `auto` would have picked is only named.
            if frontier.is_some_and(|f| f.host_is_zai())
                && let Ok(mut c) = zai(cfg, frontier, env)?
            {
                c.detail = format!(
                    "{} [auto: the frontier's own search; web.search.backend = \"native\" asks \
public sources directly instead]",
                    c.detail
                );
                return Ok(c);
            }
            let mut unused = Vec::new();
            if !cfg.str("web.search.searxng_url")?.trim().is_empty() {
                unused.push("web.search.searxng_url is set: web.search.backend = \"searxng\" searches with it");
            }
            let brave_env = cfg.str("web.search.brave_key_env")?;
            if var(env, &brave_env).is_some() {
                unused.push("a Brave key is set: web.search.backend = \"brave\" searches with it");
            }
            if cfg.str("web.search.zai_engine")? != "auto" {
                unused.push(
                    "web.search.zai_engine is set: web.search.backend = \"zai\" searches with Z.ai",
                );
            }
            native(cfg, ws, Some(&unused.join("; ")))?
        }
    })
}

/// The package registries of the languages found at the workspace root.
pub fn registries(ws: &Path) -> Vec<Source> {
    let has = |names: &[&str]| names.iter().any(|n| ws.join(n).is_file());
    let mut out = Vec::new();
    if has(&["Cargo.toml"]) {
        out.push(Source::Crates);
    }
    if has(&["package.json"]) {
        out.push(Source::Npm);
    }
    if has(&[
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "requirements.txt",
        "Pipfile",
    ]) {
        out.push(Source::PyPi);
    }
    out
}

/// The sources `auto` asks by default: Stack Overflow, Wikipedia, GitHub
/// repositories and the workspace's registries.
pub fn auto_sources(ws: &Path) -> Vec<Source> {
    let mut out = vec![Source::StackOverflow, Source::Wikipedia, Source::GitHub];
    out.extend(registries(ws));
    out
}

/// The native backend's sources from `web.search.sources`: `auto` (the
/// defaults above, every other source on request) and names (asked by
/// default). Without `auto`, only the named sources.
pub fn native_search(cfg: &Config, ws: &Path) -> Result<NativeSearch> {
    let entries = cfg.list("web.search.sources")?;
    let mut defaults = Vec::new();
    let mut auto = false;
    for e in &entries {
        match e.trim() {
            "auto" => auto = true,
            name => match Source::from_name(name) {
                Some(s) => defaults.push(s),
                None => {
                    let known: Vec<&str> = ALL.iter().map(|s| s.name()).collect();
                    bail!(
                        "web.search.sources: unknown source {name:?} (auto, {})",
                        known.join(", ")
                    );
                }
            },
        }
    }
    let on_request: Vec<Source> = if auto {
        defaults.extend(auto_sources(ws));
        ALL.iter()
            .copied()
            .filter(|s| !defaults.contains(s))
            .collect()
    } else {
        Vec::new()
    };
    Ok(NativeSearch::new(&defaults, &on_request))
}

fn hosts_of<'a>(
    sources: impl Iterator<Item = &'a SourceSetup>,
) -> (Vec<&'static str>, Vec<String>) {
    let sources: Vec<&SourceSetup> = sources.collect();
    (
        sources.iter().map(|s| s.source.name()).collect(),
        NativeSearch::hosts(&sources),
    )
}

/// The native backend, or `web_search` off when it has no sources.
/// `unused` names backends an earlier `auto` would have picked.
fn native(cfg: &Config, ws: &Path, unused: Option<&str>) -> Result<Choice> {
    let search = native_search(cfg, ws)?;
    if search.defaults().next().is_none() {
        return Ok(off("web.search.sources names no source".into()));
    }
    let (names, hosts) = hosts_of(search.defaults());
    let mut detail = format!(
        "native: this machine asks public sources directly, no search provider in between; each \
source asked receives the query and this machine's address. By default {} ({})",
        names.join(", "),
        hosts.join(", ")
    );
    let (names, hosts) = hosts_of(search.on_request());
    if !names.is_empty() {
        detail.push_str(&format!(
            "; when the frontier names them {} ({})",
            names.join(", "),
            hosts.join(", ")
        ));
    }
    if let Some(unused) = unused.filter(|u| !u.is_empty()) {
        detail.push_str(&format!(" [auto searches natively; {unused}]"));
    }
    Ok(chosen(Backend::Native(search), detail))
}

/// Z.ai's search with the frontier's key when the frontier is Z.ai: the
/// coding plan's search server for a coding-plan frontier, the per-search
/// Web Search API otherwise (`web.search.zai_engine` decides).
fn zai(
    cfg: &Config,
    frontier: Option<Frontier<'_>>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Result<Choice, String>> {
    let own = frontier.filter(Frontier::host_is_zai);
    let key_env = own.map_or(ZAI_KEY_ENV, |f| f.key_env);
    let Some(key) = var(env, key_env) else {
        return Ok(Err(format!("${key_env} is not set")));
    };
    let engine = cfg.str("web.search.zai_engine")?;
    let plan = match engine.as_str() {
        "plan" => true,
        "auto" => own.is_some_and(|f| f.coding_plan()),
        _ => false,
    };
    let recipient = if own.is_some() {
        "the frontier provider, which already receives the run"
    } else {
        "Z.ai, which is not this run's frontier: a further recipient"
    };
    let choice = if plan {
        chosen(
            Backend::ZaiPlan {
                endpoint: url::Url::parse(ZAI_PLAN_ENDPOINT)?,
                key,
            },
            format!(
                "zai: the GLM Coding Plan's search server (counted in the plan's credits; key from \
${key_env}); queries go to {recipient}"
            ),
        )
    } else {
        let engine = if engine == "auto" {
            ZAI_ENGINES[0].to_owned()
        } else {
            engine
        };
        let detail = format!(
            "zai: the Web Search API with {engine} (billed per search to the Z.ai account balance, \
not the coding plan; key from ${key_env}); queries go to {recipient}"
        );
        chosen(
            Backend::Zai {
                endpoint: url::Url::parse(ZAI_ENDPOINT)?,
                key,
                engine,
            },
            detail,
        )
    };
    Ok(Ok(choice))
}

/// The owner's SearXNG instance, or `None` when no URL is set.
fn searxng(cfg: &Config) -> Result<Option<Choice>> {
    let raw = cfg.str("web.search.searxng_url")?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let base = url::Url::parse(raw.trim()).context("web.search.searxng_url")?;
    if !matches!(base.scheme(), "http" | "https") {
        bail!("web.search.searxng_url must be an http or https URL");
    }
    let detail = format!(
        "searxng at {base} (your instance; it passes queries on to the search engines it is set \
up with, without your identity)"
    );
    Ok(Some(chosen(Backend::Searxng { base }, detail)))
}

fn brave(cfg: &Config, env: &dyn Fn(&str) -> Option<String>) -> Result<Result<Choice, String>> {
    let key_env = cfg.str("web.search.brave_key_env")?;
    let Some(key) = var(env, &key_env) else {
        return Ok(Err(format!("${key_env} is not set")));
    };
    Ok(Ok(chosen(
        Backend::Brave {
            endpoint: url::Url::parse(BRAVE_ENDPOINT)?,
            key,
        },
        format!("brave: the Brave Search API (key from ${key_env}); queries go to Brave"),
    )))
}

fn wikipedia(why: &str) -> Choice {
    chosen(
        Backend::Wikipedia {
            endpoint: url::Url::parse(WIKIPEDIA_ENDPOINT).expect("a valid constant"),
        },
        format!(
            "wikipedia: English Wikipedia articles only, no key ({why}); queries go to the \
Wikimedia Foundation"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const CODING: &str = "https://api.z.ai/api/coding/paas/v4";

    fn config(toml: &str) -> (tempfile::TempDir, Config) {
        let d = tempfile::tempdir().unwrap();
        let owner = d.path().join("config.toml");
        std::fs::write(&owner, toml).unwrap();
        let cfg = Config::load(&owner, None).unwrap();
        (d, cfg)
    }

    fn pick_in(
        ws: &Path,
        toml: &str,
        frontier: Option<Frontier<'_>>,
        vars: &[(&str, &str)],
    ) -> Choice {
        let (_d, cfg) = config(toml);
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        choose(&cfg, frontier, &|name| vars.get(name).cloned(), ws).unwrap()
    }

    fn pick(toml: &str, frontier: Option<Frontier<'_>>, vars: &[(&str, &str)]) -> Choice {
        let ws = tempfile::tempdir().unwrap();
        pick_in(ws.path(), toml, frontier, vars)
    }

    fn zai_frontier(base_url: &str) -> Option<Frontier<'_>> {
        Some(Frontier {
            base_url,
            key_env: "ZAI_API_KEY",
        })
    }

    fn name(c: &Choice) -> Option<&'static str> {
        c.backend.as_ref().map(Backend::name)
    }

    fn defaults(c: &Choice) -> Vec<&'static str> {
        match &c.backend {
            Some(Backend::Native(n)) => n.defaults().map(|s| s.source.name()).collect(),
            other => panic!("not native: {other:?}"),
        }
    }

    #[test]
    fn auto_uses_the_frontiers_own_search_else_searches_natively() {
        let key = [("ZAI_API_KEY", "zk-1"), ("BRAVE_API_KEY", "bk-1")];
        // The default setup (the coding plan's frontier and its key): Z.ai's
        // search, its queries going only to the provider of the run.
        let c = pick("", zai_frontier(CODING), &key);
        assert_eq!(name(&c), Some("zai (coding plan)"), "{c:?}");
        assert!(c.problem.is_none());
        for want in [
            "the frontier provider, which already receives the run",
            "[auto: the frontier's own search",
        ] {
            assert!(c.detail.contains(want), "{want}: {}", c.detail);
        }
        assert!(!c.detail.contains("zk-1") && !c.detail.contains("bk-1"));
        // Without its key, another frontier, a run without one: native.
        let anthropic = Some(Frontier {
            base_url: "https://api.anthropic.com/v1",
            key_env: "ANTHROPIC_API_KEY",
        });
        for (frontier, vars) in [
            (zai_frontier(CODING), &key[1..]),
            (anthropic, &key[..]),
            (None, &key[..]),
        ] {
            let c = pick("", frontier, vars);
            assert_eq!(name(&c), Some("native"), "{c:?}");
            assert!(c.problem.is_none());
            assert_eq!(defaults(&c), ["stackoverflow", "wikipedia", "github"]);
            for want in [
                "no search provider in between",
                "each source asked receives the query",
                "api.stackexchange.com, en.wikipedia.org, api.github.com",
                "when the frontier names them serverfault",
                "arxiv",
            ] {
                assert!(c.detail.contains(want), "{want}: {}", c.detail);
            }
            assert!(!c.detail.contains("zk-1") && !c.detail.contains("bk-1"));
        }
        // A SearXNG URL or a Brave key is named, not used.
        let c = pick(
            "[web.search]\nsearxng_url = \"http://127.0.0.1:8888\"\n",
            anthropic,
            &key,
        );
        assert_eq!(name(&c), Some("native"));
        assert!(
            c.detail
                .contains("web.search.backend = \"searxng\" searches with it")
                && c.detail.contains("web.search.backend = \"brave\""),
            "{}",
            c.detail
        );
        let c = pick("", None, &[]);
        assert!(!c.detail.contains("auto searches natively"), "{}", c.detail);
        // `native` by name keeps a Z.ai frontier's runs off Z.ai's search.
        let c = pick(
            "[web.search]\nbackend = \"native\"\n",
            zai_frontier(CODING),
            &key,
        );
        assert_eq!(name(&c), Some("native"));
    }

    #[test]
    fn the_workspace_languages_add_their_registries() {
        let ws = tempfile::tempdir().unwrap();
        for f in ["Cargo.toml", "package.json", "pyproject.toml"] {
            std::fs::write(ws.path().join(f), "").unwrap();
        }
        let c = pick_in(ws.path(), "", None, &[]);
        assert_eq!(
            defaults(&c),
            [
                "stackoverflow",
                "wikipedia",
                "github",
                "crates",
                "npm",
                "pypi"
            ]
        );
        assert!(
            c.detail.contains("crates.io, registry.npmjs.org, pypi.org"),
            "{}",
            c.detail
        );
        let only_python = tempfile::tempdir().unwrap();
        std::fs::write(only_python.path().join("requirements.txt"), "").unwrap();
        assert_eq!(registries(only_python.path()), [Source::PyPi]);
        // A directory named like a manifest is not one.
        let odd = tempfile::tempdir().unwrap();
        std::fs::create_dir(odd.path().join("Cargo.toml")).unwrap();
        assert!(registries(odd.path()).is_empty());
    }

    #[test]
    fn web_search_sources_pick_the_defaults_and_what_may_be_asked() {
        // Named sources are asked by default; with `auto`, next to its own.
        let c = pick(
            "[web.search]\nsources = [\"auto\", \"github_issues\"]\n",
            None,
            &[],
        );
        assert_eq!(
            defaults(&c),
            ["stackoverflow", "wikipedia", "github", "github_issues"]
        );
        // Without `auto`, only the named ones, and nothing on request.
        let c = pick(
            "[web.search]\nsources = [\"wikipedia\", \"arxiv\"]\n",
            None,
            &[],
        );
        assert_eq!(defaults(&c), ["wikipedia", "arxiv"]);
        match &c.backend {
            Some(Backend::Native(n)) => assert_eq!(n.on_request().count(), 0),
            other => panic!("{other:?}"),
        }
        assert!(
            !c.detail.contains("when the frontier names them"),
            "{}",
            c.detail
        );
        // No source: no web_search; an unknown one is an error.
        let c = pick("[web.search]\nsources = []\n", None, &[]);
        assert!(c.backend.is_none());
        assert!(c.problem.unwrap().contains("names no source"));
        let (_d, cfg) = config("[web.search]\nsources = [\"google\"]\n");
        let ws = tempfile::tempdir().unwrap();
        let e = choose(&cfg, None, &|_| None, ws.path()).unwrap_err();
        assert!(e.to_string().contains("unknown source \"google\""), "{e}");
    }

    #[test]
    fn explicit_backends_are_used_or_reported_and_none_disables() {
        let c = pick(
            "[web.search]\nbackend = \"none\"\n",
            zai_frontier(CODING),
            &[("ZAI_API_KEY", "zk")],
        );
        assert!(c.backend.is_none() && c.problem.is_none());

        let c = pick("[web.search]\nbackend = \"brave\"\n", None, &[]);
        assert!(c.backend.is_none());
        assert!(c.problem.unwrap().contains("$BRAVE_API_KEY is not set"));
        let c = pick(
            "[web.search]\nbackend = \"brave\"\n",
            None,
            &[("BRAVE_API_KEY", "bk")],
        );
        assert_eq!(name(&c), Some("brave"));
        let c = pick("[web.search]\nbackend = \"searxng\"\n", None, &[]);
        assert!(c.problem.unwrap().contains("searxng_url is empty"));
        let c = pick(
            "[web.search]\nbackend = \"searxng\"\nsearxng_url = \"http://127.0.0.1:8888\"\n",
            None,
            &[],
        );
        assert_eq!(name(&c), Some("searxng"));
        let c = pick("[web.search]\nbackend = \"wikipedia\"\n", None, &[]);
        assert_eq!(name(&c), Some("wikipedia"));

        // Z.ai only by name: the coding plan's search with the frontier's key.
        let zai = "[web.search]\nbackend = \"zai\"\n";
        let c = pick(zai, zai_frontier(CODING), &[("ZAI_API_KEY", "zk-1")]);
        assert_eq!(name(&c), Some("zai (coding plan)"));
        assert!(
            c.detail.contains("already receives the run"),
            "{}",
            c.detail
        );
        let c = pick(
            zai,
            zai_frontier("https://api.z.ai/api/paas/v4"),
            &[("ZAI_API_KEY", "zk-1")],
        );
        assert!(
            matches!(&c.backend, Some(Backend::Zai { engine, .. }) if engine == "search_pro_jina"),
            "{c:?}"
        );
        assert!(c.detail.contains("billed per search"));

        // An explicit zai with another frontier uses ZAI_API_KEY, never the
        // frontier's key, and says Z.ai is a further recipient.
        let openai = Some(Frontier {
            base_url: "https://api.openai.com/v1",
            key_env: "OPENAI_API_KEY",
        });
        let c = pick(zai, openai, &[("OPENAI_API_KEY", "ok-1")]);
        assert!(c.problem.unwrap().contains("$ZAI_API_KEY is not set"));
        let c = pick(
            zai,
            openai,
            &[("OPENAI_API_KEY", "ok-1"), ("ZAI_API_KEY", "zk-2")],
        );
        match &c.backend {
            Some(Backend::Zai { key, engine, .. }) => {
                assert_eq!(key, "zk-2");
                assert_eq!(engine, "search_pro_jina");
            }
            other => panic!("{other:?}"),
        }
        assert!(c.detail.contains("further recipient"), "{}", c.detail);

        // The engine setting picks the plan or an API engine.
        let c = pick(
            "[web.search]\nbackend = \"zai\"\nzai_engine = \"search-prime\"\n",
            zai_frontier(CODING),
            &[("ZAI_API_KEY", "zk")],
        );
        assert!(
            matches!(&c.backend, Some(Backend::Zai { engine, .. }) if engine == "search-prime"),
            "{c:?}"
        );
        let c = pick(
            "[web.search]\nbackend = \"zai\"\nzai_engine = \"plan\"\n",
            zai_frontier("https://api.z.ai/api/paas/v4"),
            &[("ZAI_API_KEY", "zk")],
        );
        assert_eq!(name(&c), Some("zai (coding plan)"));
        // The engine alone does not select Z.ai (without a Z.ai frontier).
        let c = pick(
            "[web.search]\nzai_engine = \"plan\"\n",
            None,
            &[("ZAI_API_KEY", "zk")],
        );
        assert_eq!(name(&c), Some("native"));
    }

    #[test]
    fn a_searxng_url_must_be_http() {
        let (_d, cfg) =
            config("[web.search]\nbackend = \"searxng\"\nsearxng_url = \"file:///etc/passwd\"\n");
        let ws = tempfile::tempdir().unwrap();
        assert!(choose(&cfg, None, &|_| None, ws.path()).is_err());
    }
}
