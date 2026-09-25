// SPDX-License-Identifier: GPL-3.0-or-later
//! Owner setup: audited owner-config changes, local backend presets, and the
//! no-config bootstrap that finds a local server on loopback for one run.

use crate::config_audit_path;
use anyhow::Result;
use duet_boundary::audit::record_config_change;
use duet_config::{Config, Origin, Target};
use duet_provider::backends::{self, PRESETS, Pick, Server};
use std::time::Duration;
use toml::Value;

/// Applies owner-config changes through the loosening rules: when any of them
/// loosens privacy and `confirm` is not given, every loosening change is shown
/// and nothing is applied (exit code 2). Each applied change is recorded in the
/// owner's config audit log.
pub fn apply_owner(cfg: &mut Config, changes: &[(&str, Value)], confirm: bool) -> Result<i32> {
    let mut refused = Vec::new();
    for (key, new) in changes {
        let p = cfg.propose(Target::Owner, key, new.clone())?;
        if let Some(weakens) = &p.weakens {
            refused.push(format!(
                "  {key}\n  - {}\n  + {}\n\n  weakens: {weakens}\n",
                p.old, p.new
            ));
        }
    }
    if !refused.is_empty() && !confirm {
        eprintln!(
            "policy change not applied: it loosens privacy\n\n{}\nRe-run with --confirm to apply it; the change is recorded in {}.",
            refused.join("\n"),
            config_audit_path().display()
        );
        return Ok(2);
    }
    for (key, new) in changes {
        let change = cfg.apply(Target::Owner, key, new.clone(), confirm)?;
        record_config_change(&config_audit_path(), &change, Target::Owner, confirm)?;
        println!("{key} = {}", cfg.value(key)?);
    }
    Ok(0)
}

/// `duet config preset`: without a name, the preset table; with one, the
/// owner's `local.base_url` (and `local.model` when given) for that backend.
pub fn preset(
    cfg: &mut Config,
    name: Option<&str>,
    model: Option<&str>,
    port: Option<u16>,
    confirm: bool,
) -> Result<i32> {
    let Some(name) = name else {
        println!(
            "{:<9} {:<20} {:<26} models / context window",
            "preset", "backend", "local.base_url"
        );
        for p in PRESETS {
            println!(
                "{:<9} {:<20} {:<26} {}\n{:<57} {}",
                p.name,
                p.label,
                p.base_url(None),
                p.models,
                "",
                p.context
            );
        }
        println!(
            "\nApply one with: duet config preset <name> [--model <id>] [--port <n>] --confirm"
        );
        return Ok(0);
    };
    let Some(p) = backends::preset(name) else {
        let names: Vec<_> = PRESETS.iter().map(|p| p.name).collect();
        anyhow::bail!("unknown preset {name}; one of {}", names.join(", "));
    };
    let mut changes = vec![("local.base_url", Value::String(p.base_url(port)))];
    if let Some(m) = model {
        changes.push(("local.model", Value::String(m.to_owned())));
    }
    let code = apply_owner(cfg, &changes, confirm)?;
    if code == 0 && model.is_none() {
        println!(
            "local.model is still {}. List the server's models with `duet doctor --online`, then: duet config set local.model '\"<id>\"' (for {}: {})",
            cfg.value("local.model")?,
            p.label,
            p.example_model
        );
    }
    Ok(code)
}

/// The local endpoint chosen for one run when the owner configured none.
pub struct Bootstrap {
    pub base_url: String,
    pub model: String,
}

/// Loopback ports bootstrap probes: `DUET_LOCAL_PORTS` (comma-separated) or the preset ports.
pub fn bootstrap_ports() -> Vec<u16> {
    std::env::var("DUET_LOCAL_PORTS")
        .ok()
        .map(|v| v.split(',').filter_map(|p| p.trim().parse().ok()).collect())
        .unwrap_or_else(backends::loopback_ports)
}

fn describe(s: &Server) -> String {
    format!(
        "{} at {}: {}",
        s.backend.unwrap_or("OpenAI-compatible server"),
        s.base_url,
        if s.models.is_empty() {
            "no models listed".to_owned()
        } else {
            s.models.join(", ")
        }
    )
}

fn set_commands(base_url: &str, model: &str) -> String {
    format!(
        "  duet config set local.base_url '\"{base_url}\"' --confirm\n  duet config set local.model '\"{model}\"'"
    )
}

/// When the owner has not configured a local endpoint, looks for a server on
/// loopback. Returns `Ok(None)` when configuration applies, `Ok(Some(..))` to
/// use a single unambiguous server for this run, or `Err(code)` after printing
/// the commands that would configure one. Never writes configuration.
pub async fn bootstrap(cfg: &Config) -> Result<Option<Bootstrap>, i32> {
    if cfg.origin("local.base_url") != Some(Origin::Default) {
        return Ok(None);
    }
    let ports = bootstrap_ports();
    let list: Vec<String> = ports.iter().map(u16::to_string).collect();
    eprintln!(
        "no local model endpoint is configured; looking for a local server on 127.0.0.1 port(s) {}",
        list.join(", ")
    );
    let servers = backends::discover_loopback(&ports, Duration::from_millis(1500)).await;
    for s in &servers {
        eprintln!("  found {}", describe(s));
    }
    let preferred = cfg.str("local.model").ok();
    match backends::pick(&servers, preferred.as_deref()) {
        Pick::One { server, model } => {
            eprintln!(
                "using {model} at {} for this run only (configuration unchanged). To keep it:\n{}",
                server.base_url,
                set_commands(&server.base_url, &model)
            );
            Ok(Some(Bootstrap {
                base_url: server.base_url.clone(),
                model,
            }))
        }
        Pick::Ambiguous => {
            eprintln!("\nmore than one local model is available; choose one:");
            for s in servers.iter().filter(|s| !s.models.is_empty()) {
                for m in &s.models {
                    eprintln!("{}\n", set_commands(&s.base_url, m));
                }
            }
            Err(2)
        }
        Pick::Nothing => {
            eprintln!(
                "\nno local model server answered. Start one (Ollama, LM Studio, llama.cpp, vLLM, oMLX, mlx_lm.server), \
or point Duet at yours:\n  duet config preset            # list backends and their default ports\n  \
duet config preset <name> --model <id> --confirm\n  duet config set local.base_url '\"http://127.0.0.1:<port>/v1\"' --confirm"
            );
            Err(2)
        }
    }
}
