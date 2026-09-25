// SPDX-License-Identifier: GPL-3.0-or-later
//! The run's web access, from the `web.*` settings.

use anyhow::{Context, Result, bail};
use duet_config::Config;
use duet_web::guard::Allowlist;
use duet_web::search::{BRAVE_ENDPOINT, Backend};
use duet_web::{Web, WebConfig};
use std::sync::Arc;
use std::time::Duration;

/// Web access for this run, or `None` when `web.enabled` is off. A search
/// backend that cannot be used (no URL, no key in the environment) leaves
/// `web_search` out with a warning; `web_fetch` is still offered.
pub fn access(cfg: &Config) -> Result<Option<Arc<Web>>> {
    if !cfg.bool("web.enabled")? {
        return Ok(None);
    }
    let allowlist = Allowlist::parse(&cfg.list("web.allowlist_private")?)
        .map_err(anyhow::Error::msg)
        .context("web.allowlist_private")?;
    Ok(Some(Arc::new(Web::new(WebConfig {
        max_bytes: cfg.int("web.max_bytes")? as usize,
        timeout: Duration::from_secs(cfg.int("web.timeout_secs")? as u64),
        allowlist,
        search: backend(cfg)?,
    }))))
}

fn backend(cfg: &Config) -> Result<Option<Backend>> {
    match cfg.str("web.search.backend")?.as_str() {
        "searxng" => {
            let raw = cfg.str("web.search.searxng_url")?;
            if raw.trim().is_empty() {
                eprintln!(
                    "warning: web.search.backend is searxng but web.search.searxng_url is empty; web_search is off"
                );
                return Ok(None);
            }
            let base = url::Url::parse(raw.trim()).context("web.search.searxng_url")?;
            if !matches!(base.scheme(), "http" | "https") {
                bail!("web.search.searxng_url must be an http or https URL");
            }
            Ok(Some(Backend::Searxng { base }))
        }
        "brave" => {
            let var = cfg.str("web.search.brave_key_env")?;
            match std::env::var(&var) {
                Ok(key) if !key.trim().is_empty() => Ok(Some(Backend::Brave {
                    endpoint: url::Url::parse(BRAVE_ENDPOINT)?,
                    key: key.trim().to_owned(),
                })),
                _ => {
                    eprintln!(
                        "warning: web.search.backend is brave but ${var} is not set; web_search is off"
                    );
                    Ok(None)
                }
            }
        }
        _ => Ok(None),
    }
}
