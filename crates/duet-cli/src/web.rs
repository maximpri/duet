// SPDX-License-Identifier: GPL-3.0-or-later
//! The run's web access, from the `web.*` settings, and the choice of search
//! backend (`web.search.backend`, `auto` by default): Z.ai when the frontier
//! is Z.ai and its key is set (the queries go to the provider that already
//! receives the frontier traffic), else the owner's SearXNG instance, else
//! Brave when its key is set, else Wikipedia (no key, no setup).

use anyhow::{Context, Result, bail};
use duet_config::Config;
use duet_web::guard::Allowlist;
use duet_web::search::{
    BRAVE_ENDPOINT, Backend, WIKIPEDIA_ENDPOINT, ZAI_ENDPOINT, ZAI_ENGINES, ZAI_PLAN_ENDPOINT,
};
use duet_web::{Web, WebConfig};
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
/// is the run's frontier (`None` in local-only mode). A search backend that
/// cannot be used leaves `web_search` out with a warning; `web_fetch` is still
/// offered.
pub fn access(cfg: &Config, frontier: Option<Frontier<'_>>) -> Result<Option<Arc<Web>>> {
    if !cfg.bool("web.enabled")? {
        return Ok(None);
    }
    let allowlist = Allowlist::parse(&cfg.list("web.allowlist_private")?)
        .map_err(anyhow::Error::msg)
        .context("web.allowlist_private")?;
    let choice = choose(cfg, frontier, &|name| std::env::var(name).ok())?;
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

/// The search backend from the settings, the run's frontier and the
/// environment (`env` looks variables up; keys are read, never printed).
pub fn choose(
    cfg: &Config,
    frontier: Option<Frontier<'_>>,
    env: &dyn Fn(&str) -> Option<String>,
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
        _ => {
            // `auto`: Z.ai only as the run's own frontier, never as a new
            // recipient.
            if frontier.is_some_and(|f| f.host_is_zai())
                && let Ok(c) = zai(cfg, frontier, env)?
            {
                return Ok(c);
            }
            if let Some(c) = searxng(cfg)? {
                return Ok(c);
            }
            if let Ok(c) = brave(cfg, env)? {
                return Ok(c);
            }
            wikipedia(
                "no other search backend is available (auto); for whole-web search set up \
a private SearXNG (`duet config preset searxng`)",
            )
        }
    })
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

    fn pick(toml: &str, frontier: Option<Frontier<'_>>, vars: &[(&str, &str)]) -> Choice {
        let (_d, cfg) = config(toml);
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        choose(&cfg, frontier, &|name| vars.get(name).cloned()).unwrap()
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

    #[test]
    fn auto_prefers_the_zai_frontier_then_searxng_then_brave_then_wikipedia() {
        let key = [("ZAI_API_KEY", "zk-1")];
        // The default setup: the coding plan's frontier and its key.
        let c = pick("", zai_frontier(CODING), &key);
        assert_eq!(name(&c), Some("zai (coding plan)"));
        assert!(
            c.detail.contains("already receives the run"),
            "{}",
            c.detail
        );
        assert!(!c.detail.contains("zk-1"));
        // A pay-as-you-go frontier searches with the Web Search API.
        let c = pick("", zai_frontier("https://api.z.ai/api/paas/v4"), &key);
        assert!(
            matches!(&c.backend, Some(Backend::Zai { engine, .. }) if engine == "search_pro_jina"),
            "{c:?}"
        );
        assert!(c.detail.contains("billed per search"));

        let searx = "[web.search]\nsearxng_url = \"http://127.0.0.1:8888\"\n";
        let brave = [("BRAVE_API_KEY", "bk-1")];
        // Without the key, or with another frontier, the next one is tried.
        assert_eq!(
            name(&pick(searx, zai_frontier(CODING), &brave)),
            Some("searxng")
        );
        assert_eq!(name(&pick("", zai_frontier(CODING), &brave)), Some("brave"));
        let anthropic = Some(Frontier {
            base_url: "https://api.anthropic.com/v1",
            key_env: "ANTHROPIC_API_KEY",
        });
        assert_eq!(
            name(&pick(
                "",
                anthropic,
                &[("ANTHROPIC_API_KEY", "ak"), ("ZAI_API_KEY", "zk")]
            )),
            Some("wikipedia")
        );
        // Local-only runs have no frontier: Z.ai would be a new recipient.
        assert_eq!(name(&pick("", None, &key)), Some("wikipedia"));
        // Nothing configured at all: the keyless fallback.
        let c = pick("", zai_frontier(CODING), &[("ZAI_API_KEY", "  ")]);
        assert_eq!(name(&c), Some("wikipedia"));
        assert!(
            c.problem.is_none() && c.detail.contains("Wikimedia"),
            "{c:?}"
        );
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
        let c = pick("[web.search]\nbackend = \"searxng\"\n", None, &[]);
        assert!(c.problem.unwrap().contains("searxng_url is empty"));
        let c = pick("[web.search]\nbackend = \"wikipedia\"\n", None, &[]);
        assert_eq!(name(&c), Some("wikipedia"));

        // An explicit zai with another frontier uses ZAI_API_KEY, never the
        // frontier's key, and says Z.ai is a further recipient.
        let openai = Some(Frontier {
            base_url: "https://api.openai.com/v1",
            key_env: "OPENAI_API_KEY",
        });
        let zai = "[web.search]\nbackend = \"zai\"\n";
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
            "[web.search]\nzai_engine = \"plan\"\n",
            zai_frontier("https://api.z.ai/api/paas/v4"),
            &[("ZAI_API_KEY", "zk")],
        );
        assert_eq!(name(&c), Some("zai (coding plan)"));
    }

    #[test]
    fn a_searxng_url_must_be_http() {
        let (_d, cfg) =
            config("[web.search]\nbackend = \"searxng\"\nsearxng_url = \"file:///etc/passwd\"\n");
        assert!(choose(&cfg, None, &|_| None).is_err());
    }
}
