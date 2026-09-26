// SPDX-License-Identifier: GPL-3.0-or-later
//! Owner setup: audited owner-config changes, local backend, frontier and
//! private-search presets, and the no-config bootstrap that finds a local
//! server on loopback for one run.

use crate::config_audit_path;
use anyhow::Result;
use duet_boundary::audit::record_config_change;
use duet_config::{Config, Origin, Target};
use duet_provider::backends::{self, FRONTIER_PRESETS, PRESETS, Pick, Server};
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

/// `duet config preset`: without a name, the preset tables; with a local
/// backend's name, the owner's `local.base_url` (and `local.model` when given);
/// with a frontier's name, the owner's `frontier.base_url`, `frontier.model`
/// (the preset's unless `--model` is given), `frontier.api_key_env`,
/// `frontier.dialect` and `frontier.vision` (whether the preset's model takes
/// images). Every change goes through the audited owner-config path.
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
            "\n{:<9} {:<24} {:<10} {:<18} frontier.base_url / model",
            "preset", "frontier", "dialect", "key variable"
        );
        for p in FRONTIER_PRESETS {
            println!(
                "{:<9} {:<24} {:<10} {:<18} {} / {}",
                p.name, p.label, p.dialect, p.api_key_env, p.base_url, p.model
            );
        }
        println!(
            "\n{:<9} private web search: a SearXNG instance in Docker or OrbStack on 127.0.0.1:{SEARXNG_PORT} \
(prints the commands; runs nothing)",
            "searxng"
        );
        println!(
            "\nApply one with: duet config preset <name> [--model <id>] [--port <n>] --confirm\n\
(--port applies to local backends and searxng)"
        );
        return Ok(0);
    };
    if name.eq_ignore_ascii_case("searxng") {
        anyhow::ensure!(model.is_none(), "--model does not apply to searxng");
        return searxng(cfg, port.unwrap_or(SEARXNG_PORT), confirm);
    }
    if let Some(f) = backends::frontier_preset(name) {
        anyhow::ensure!(
            port.is_none(),
            "--port applies to local backends; {name} is a frontier preset"
        );
        let changes = [
            ("frontier.base_url", Value::String(f.base_url.to_owned())),
            (
                "frontier.model",
                Value::String(model.unwrap_or(f.model).to_owned()),
            ),
            (
                "frontier.api_key_env",
                Value::String(f.api_key_env.to_owned()),
            ),
            ("frontier.dialect", Value::String(f.dialect.to_owned())),
            ("frontier.vision", Value::Boolean(f.vision)),
        ];
        let code = apply_owner(cfg, &changes, confirm)?;
        if code == 0 {
            if std::env::var(f.api_key_env).map_or(true, |k| k.is_empty()) {
                println!(
                    "{} is not set in this environment; export it before `duet run` (Duet stores only the variable's name).",
                    f.api_key_env
                );
            }
            if duet_provider::price::builtin(&cfg.str("frontier.model")?).is_none() {
                println!(
                    "no list price is known for {}: limits.frontier_usd cannot be enforced for it.",
                    cfg.value("frontier.model")?
                );
            }
        }
        return Ok(code);
    }
    let Some(p) = backends::preset(name) else {
        let names: Vec<_> = PRESETS
            .iter()
            .map(|p| p.name)
            .chain(FRONTIER_PRESETS.iter().map(|p| p.name))
            .collect();
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

/// The host port of the local SearXNG the `searxng` preset sets up.
pub const SEARXNG_PORT: u16 = 8888;

/// SearXNG's settings for Duet: its defaults, plus the JSON format that
/// `web_search` reads and no rate limiter (one user, on loopback).
fn searxng_settings(secret: &str) -> String {
    format!(
        "# SearXNG settings written by `duet config preset searxng`: SearXNG's defaults, plus the\n\
# JSON format Duet's web_search reads. The container mounts this folder at /etc/searxng.\n\
use_default_settings: true\n\
server:\n  secret_key: \"{secret}\"\n  limiter: false\n  image_proxy: false\n\
search:\n  formats:\n    - html\n    - json\n"
    )
}

/// `duet config preset searxng`: points `web_search` at a SearXNG instance
/// on `127.0.0.1:<port>` (through the audited owner-config path), writes its
/// settings next to the owner config when there are none, and prints the
/// command that starts it. Starting containers is left to the operator.
fn searxng(cfg: &mut Config, port: u16, confirm: bool) -> Result<i32> {
    let url = format!("http://127.0.0.1:{port}");
    let changes = [
        ("web.search.backend", Value::String("searxng".into())),
        ("web.search.searxng_url", Value::String(url.clone())),
    ];
    let code = apply_owner(cfg, &changes, confirm)?;
    if code != 0 {
        return Ok(code);
    }
    let dir = cfg
        .owner_path
        .parent()
        .map_or_else(
            || std::path::PathBuf::from("."),
            std::path::Path::to_path_buf,
        )
        .join("searxng");
    let settings = dir.join("settings.yml");
    if settings.exists() {
        println!("kept {} (not overwritten)", settings.display());
    } else {
        std::fs::create_dir_all(&dir)?;
        let secret = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        std::fs::write(&settings, searxng_settings(&secret))?;
        println!("wrote {} (JSON format on)", settings.display());
    }
    println!(
        "\nDuet does not start containers. Start SearXNG yourself (Docker or OrbStack), \
listening on loopback only:\n\n  docker run -d --name duet-searxng --restart unless-stopped \\\n    \
-p 127.0.0.1:{port}:8080 \\\n    -v \"{}:/etc/searxng\" \\\n    docker.io/searxng/searxng:latest\n\n\
Then check it answers: duet doctor --online\n\
Queries go from your machine to the search engines SearXNG is set up with, without an account \
or key; stop it with: docker rm -f duet-searxng",
        dir.display()
    );
    Ok(0)
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
