// SPDX-License-Identifier: GPL-3.0-or-later
//! Model-price selection and persisted local operating-cost accounting.
use anyhow::Result;
use duet_config::Config;
use duet_provider::catalog::{Catalog, Quote};
use duet_provider::meter::{Meter, Rates, Snapshot};
use duet_provider::price::Price;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Economics {
    pub frontier: Option<Quote>,
    pub pricing_source: String,
    pub local_rates: Rates,
    pub local: Snapshot,
}

pub(crate) fn meter(cfg: &Config) -> Result<Arc<Meter>> {
    Ok(Arc::new(Meter::new(Rates {
        input_per_million: cfg.float("local.input_usd_per_million")?,
        output_per_million: cfg.float("local.output_usd_per_million")?,
    })))
}

pub(crate) fn quote(catalog: &Catalog, cfg: &Config, model: &str, base_url: &str) -> Option<Quote> {
    if cfg.str("pricing.manual_model").ok()?.trim() == model
        && !cfg.str("pricing.manual_base_url").ok()?.is_empty()
        && cfg
            .str("pricing.manual_base_url")
            .ok()?
            .trim_end_matches('/')
            == base_url.trim_end_matches('/')
    {
        let input = cfg.float("pricing.manual_input_usd_per_million").ok()?;
        let output = cfg.float("pricing.manual_output_usd_per_million").ok()?;
        if input > 0.0 && output > 0.0 {
            let cache_read = cfg
                .float("pricing.manual_cache_read_usd_per_million")
                .ok()?;
            let cache_write = cfg
                .float("pricing.manual_cache_write_usd_per_million")
                .ok()?;
            return Some(Quote {
                price: Price {
                    model: model.into(),
                    input,
                    output,
                    cache_read: if cache_read > 0.0 { cache_read } else { input },
                    cache_write: if cache_write > 0.0 {
                        cache_write
                    } else {
                        input
                    },
                },
                fetched_at: 0,
                overrides: Vec::new(),
                source: Some("owner-supplied direct-provider rates".into()),
            });
        }
    }
    let alias = cfg.str("pricing.frontier_model").unwrap_or_default();
    catalog.quote(if alias.trim().is_empty() {
        model
    } else {
        alias.trim()
    })
}

pub(crate) async fn catalog(cfg: &Config, url: &str, offline: bool) -> (Catalog, String) {
    let root = duet_config::owner_state_dir();
    let cache = root.join("openrouter-pricing.json");
    let cached = duet_fs::read_file(
        &root,
        Path::new("openrouter-pricing.json"),
        duet_provider::catalog::MAX_BYTES as u64,
    )
    .ok()
    .and_then(|b| Catalog::parse(&b).ok());
    let now = time::OffsetDateTime::now_utc().unix_timestamp().max(0) as u64;
    let fresh = cached
        .as_ref()
        .is_some_and(|c| c.fetched_at <= now && now - c.fetched_at < 86400);
    let loopback = duet_provider::endpoint::host_port(url).is_some_and(|(host, _)| {
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    let offline = offline || loopback || cfg.bool("pricing.offline").unwrap_or(false);
    if !offline && !fresh {
        match duet_provider::catalog::fetch().await {
            Ok(catalog) => {
                if duet_fs::private::ensure_private_dir(&root).is_ok()
                    && let Ok(bytes) = serde_json::to_vec(&catalog)
                {
                    let _ = duet_fs::private::write_private(&cache, &bytes);
                }
                return (catalog, "OpenRouter live catalog".into());
            }
            Err(reason) => eprintln!("pricing: {reason}; using saved OpenRouter prices"),
        }
    }
    match cached {
        Some(catalog) => (
            catalog,
            if fresh {
                "OpenRouter cache (<24h)"
            } else {
                "OpenRouter stale cache; estimate may be outdated"
            }
            .into(),
        ),
        None => (
            Catalog::bundled(),
            "OpenRouter bundled snapshot (2026-09-30)".into(),
        ),
    }
}

pub(crate) fn read(run_dir: &Path) -> Option<Economics> {
    duet_fs::read_file(run_dir, Path::new("economics.json"), 1024 * 1024)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

pub(crate) fn attach(
    run_dir: &Path,
    meter: &Arc<Meter>,
    frontier: Option<Quote>,
    source: String,
) -> Result<Economics> {
    if let Some(previous) = read(run_dir) {
        meter.restore(previous.local);
    }
    let economics = Economics {
        frontier,
        pricing_source: source,
        local_rates: meter.rates,
        local: meter.snapshot(),
    };
    let path = run_dir.join("economics.json");
    duet_fs::private::append_line(
        &run_dir.join("pricing-history.jsonl"),
        &serde_json::to_string(&economics)?,
    )?;
    duet_fs::private::write_private(&path, &serde_json::to_vec_pretty(&economics)?)?;
    let template = economics.clone();
    meter.observe(Arc::new(move |snapshot| {
        let mut current = template.clone();
        current.local = snapshot.clone();
        match serde_json::to_vec_pretty(&current) {
            Ok(bytes) => {
                if let Err(error) = duet_fs::private::write_private(&path, &bytes) {
                    eprintln!("local cost record could not be saved: {error}");
                }
            }
            Err(error) => eprintln!("local cost record could not be encoded: {error}"),
        }
    }));
    Ok(economics)
}

pub(crate) fn status(run_dir: &Path, frontier_cost: f64) -> String {
    let Some(e) = read(run_dir) else {
        return "pricing details unavailable".into();
    };
    let unknown = if e.local.unpriced_cancelled_requests == 0 {
        String::new()
    } else {
        format!(
            "\nCanceled local requests with unknown token charges: {}.",
            e.local.unpriced_cancelled_requests
        )
    };
    format!(
        "local estimate ${:.6} · {} input / {} output tokens · rates ${:.4}/${:.4} per 1M input/output\ncombined recorded estimate ${:.6}\n{}\n{}{}\nToken estimates; provider bills and subscription fees may differ.",
        e.local.cost_usd,
        e.local.input_tokens,
        e.local.output_tokens,
        e.local_rates.input_per_million,
        e.local_rates.output_per_million,
        frontier_cost + e.local.cost_usd,
        e.frontier
            .map(|q| q.description())
            .unwrap_or_else(|| "No frontier price applied (local-only run)".into()),
        e.pricing_source,
        unknown
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use duet_provider::Usage;
    #[test]
    fn configured_local_rates_persist_and_resume_without_recharging_history() {
        let d = tempfile::tempdir().unwrap();
        let cfgfile = d.path().join("config.toml");
        std::fs::write(
            &cfgfile,
            "[local]\ninput_usd_per_million=2.0\noutput_usd_per_million=5.0\n",
        )
        .unwrap();
        let cfg = Config::load(&cfgfile, None).unwrap();
        let m = meter(&cfg).unwrap();
        let q = Catalog::bundled().quote("glm-5.3-flash");
        attach(d.path(), &m, q.clone(), "test snapshot".into()).unwrap();
        m.record(
            Usage {
                input: 1_000_000,
                output: 100_000,
                ..Usage::default()
            },
            Usage::default(),
        );
        assert!((read(d.path()).unwrap().local.cost_usd - 2.5).abs() < 1e-12);
        let resumed = meter(&cfg).unwrap();
        attach(d.path(), &resumed, q, "test snapshot".into()).unwrap();
        assert_eq!(resumed.snapshot().cost_usd, 2.5);
        resumed.record(
            Usage {
                output: 100_000,
                ..Usage::default()
            },
            Usage::default(),
        );
        assert_eq!(read(d.path()).unwrap().local.cost_usd, 3.0);
        assert_eq!(
            std::fs::read_to_string(d.path().join("pricing-history.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        assert!(status(d.path(), 1.0).contains("combined recorded estimate $4.000000"));
    }

    #[test]
    fn status_flags_canceled_local_requests_without_guessing_their_cost() {
        let d = tempfile::tempdir().unwrap();
        let m = Arc::new(Meter::new(Rates {
            input_per_million: 2.0,
            output_per_million: 5.0,
        }));
        attach(d.path(), &m, None, "test snapshot".into()).unwrap();
        m.record_cancelled(
            duet_provider::Usage::default(),
            duet_provider::Usage::default(),
        );
        let saved = read(d.path()).unwrap();
        assert_eq!(saved.local.unpriced_cancelled_requests, 1);
        assert_eq!(saved.local.cost_usd, 0.0);
        assert!(
            status(d.path(), 0.0).contains("Canceled local requests with unknown token charges: 1")
        );
    }

    #[test]
    fn owner_rate_is_bound_to_one_exact_model() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("config.toml");
        std::fs::write(&file, "[pricing]\nmanual_model='my-chat'\nmanual_base_url='https://api.z.ai/api/coding/paas/v4'\nmanual_input_usd_per_million=2.0\nmanual_output_usd_per_million=8.0\n").unwrap();
        let cfg = Config::load(&file, None).unwrap();
        let catalog = Catalog::bundled();
        let own = quote(
            &catalog,
            &cfg,
            "my-chat",
            &cfg.str("frontier.base_url").unwrap(),
        )
        .unwrap();
        assert!(own.description().contains("owner-supplied"));
        assert_eq!(own.price.cache_read, 2.0);
        assert_eq!(own.price.output, 8.0);
        assert!(
            quote(
                &catalog,
                &cfg,
                "another-unknown",
                &cfg.str("frontier.base_url").unwrap()
            )
            .is_none()
        );
        assert!(quote(&catalog, &cfg, "my-chat", "https://different.example/v1").is_none());
    }
}
