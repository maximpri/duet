// SPDX-License-Identifier: GPL-3.0-or-later
//! Owner setup: audited owner-config changes, local backend, frontier and
//! private-search presets, and the no-config bootstrap that finds a local
//! server on loopback for one run.

use crate::config_audit_path;
use anyhow::Result;
use duet_boundary::audit::record_config_change;
use duet_config::{Config, Origin, Target};
use duet_provider::backends::{self, FRONTIER_PRESETS, PRESETS, Pick, Server};
use std::io::{self, IsTerminal, Write};
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
        anyhow::ensure!(
            model.is_some() || !f.model.is_empty(),
            "{name} has no stable default model; use `duet setup --provider {name}` to discover one, or pass --model"
        );
        let mut changes = vec![
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
        if cfg.str("frontier.base_url")? != f.base_url
            && !cfg.str("pricing.manual_model")?.is_empty()
        {
            changes.push(("pricing.manual_model", Value::String(String::new())));
        }
        let code = apply_owner(cfg, &changes, confirm)?;
        if code == 0 {
            if std::env::var(f.api_key_env).map_or(true, |k| k.is_empty()) {
                println!(
                    "{} is not set in this environment; export it before `duet run` (Duet stores only the variable's name).",
                    f.api_key_env
                );
            }
            if crate::pricing::quote(
                &duet_provider::catalog::Catalog::bundled(),
                cfg,
                &cfg.str("frontier.model")?,
                f.base_url,
            )
            .is_none()
            {
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
    changes.push((
        "local.api_key_env",
        Value::String(local_key_env(p.name).unwrap_or("").into()),
    ));
    if let Some(m) = model {
        changes.push(("local.model", Value::String(m.to_owned())));
    }
    let code = apply_owner(cfg, &changes, confirm)?;
    if code == 0 && model.is_none() {
        println!(
            "local.model is still {}. Run `duet setup` to discover and choose a served model (for {}: {}).",
            cfg.value("local.model")?,
            p.label,
            p.example_model
        );
    }
    if code == 0
        && let Some(name) = local_key_env(p.name)
        && !key_present(name)
    {
        println!("{name} is not set; export the server's API key before using this backend.");
    }
    Ok(code)
}

fn local_key_env(name: &str) -> Option<&'static str> {
    match name {
        "jan" => Some("JAN_API_KEY"),
        "litellm" => Some("LITELLM_API_KEY"),
        _ => None,
    }
}

fn key_present(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.is_empty())
}

/// A single guided setup pass. Discovery only reads local loopback model
/// listings and the selected frontier's model listing. It never asks a model
/// to generate or puts a credential in configuration or terminal output.
pub struct AutoOptions {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub local_model: Option<String>,
    pub local_url: Option<String>,
    pub yes: bool,
}

fn choose_number(label: &str, options: &[String], recommended: usize, yes: bool) -> Result<usize> {
    anyhow::ensure!(!options.is_empty(), "no {label} choices available");
    if options.len() == 1 {
        return Ok(0);
    }
    eprintln!("{label} choices:");
    for (i, option) in options.iter().enumerate() {
        eprintln!(
            "  {}. {}{}",
            i + 1,
            option,
            if i == recommended { " (suggested)" } else { "" }
        );
    }
    if yes {
        return Ok(recommended);
    }
    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "choose {label} with an explicit setup option (or use an interactive terminal)"
        );
    }
    eprint!("choose {label} [Enter for {}]: ", recommended + 1);
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let answer = line.trim();
    if answer.is_empty() {
        return Ok(recommended);
    }
    let n: usize = answer
        .parse()
        .map_err(|_| anyhow::anyhow!("enter a choice number"))?;
    anyhow::ensure!((1..=options.len()).contains(&n), "choice out of range");
    Ok(n - 1)
}

fn model_score(id: &str) -> i32 {
    let id = id.to_ascii_lowercase();
    let mut score = 0;
    for (word, points) in [
        ("coder", 40),
        ("code", 30),
        ("devstral", 40),
        ("instruct", 18),
        ("sonnet", 16),
        ("sol", 15),
        ("pro", 12),
        ("flash", 8),
        ("latest", 5),
        ("preview", -20),
        ("experimental", -30),
    ] {
        if id.contains(word) {
            score += points;
        }
    }
    score
}

fn suggested(ids: &[String]) -> usize {
    ids.iter()
        .enumerate()
        .max_by_key(|(_, id)| model_score(id))
        .map_or(0, |(i, _)| i)
}

fn choose_model(
    label: &str,
    ids: &[String],
    preferred: Option<&str>,
    requested: Option<&str>,
    yes: bool,
) -> Result<String> {
    if let Some(id) = requested {
        anyhow::ensure!(
            ids.is_empty() || ids.iter().any(|candidate| candidate == id),
            "{label} model {id} is not in the server's listing"
        );
        return Ok(id.to_owned());
    }
    if let Some(id) = preferred.filter(|id| ids.iter().any(|candidate| candidate == id)) {
        return Ok(id.to_owned());
    }
    anyhow::ensure!(
        !ids.is_empty(),
        "{label} listed no suitable models; pass --model explicitly"
    );
    let mut choices = ids.to_vec();
    choices.sort_by(|a, b| model_score(b).cmp(&model_score(a)).then_with(|| a.cmp(b)));
    if choices.len() > 12 {
        eprintln!(
            "showing 12 of {} {label}s; use --model for any other listed id",
            choices.len()
        );
        choices.truncate(12);
    }
    let i = choose_number(label, &choices, 0, yes)?;
    Ok(choices[i].clone())
}

/// Guided provider/model configuration. Existing explicit settings are kept
/// unless a selection is passed. Ambiguous cloud credentials always require
/// a provider choice; `--yes` never silently chooses a data recipient.
pub async fn auto(cfg: &mut Config, options: AutoOptions) -> Result<i32> {
    let mut changes: Vec<(&str, Value)> = Vec::new();
    let select_frontier = options.provider.is_some()
        || options.model.is_some()
        || cfg.origin("frontier.base_url") == Some(Origin::Default);
    if select_frontier {
        let provider = if let Some(name) = options.provider.as_deref() {
            Some(backends::frontier_preset(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown frontier provider {name}; run `duet config preset` for names"
                )
            })?)
        } else if cfg.origin("frontier.base_url") != Some(Origin::Default) {
            let url = cfg.str("frontier.base_url")?;
            Some(FRONTIER_PRESETS
                .iter()
                .find(|p| p.base_url == url)
                .ok_or_else(|| anyhow::anyhow!("the configured frontier endpoint is custom; pass --provider to change it, or use `duet config set frontier.model` for that endpoint"))?)
        } else {
            let available: Vec<_> = FRONTIER_PRESETS
                .iter()
                .filter(|p| key_present(p.api_key_env))
                .collect();
            match available.as_slice() {
                [] => {
                    eprintln!(
                        "no frontier API key found in the environment; add one of the variables shown by `duet config preset`"
                    );
                    None
                }
                [only] => Some(*only),
                _ => {
                    let labels: Vec<_> = available
                        .iter()
                        .map(|p| format!("{} ({})", p.label, p.name))
                        .collect();
                    anyhow::ensure!(
                        !options.yes,
                        "several frontier credentials are present; select the data recipient with --provider"
                    );
                    Some(available[choose_number("frontier provider", &labels, 0, false)?])
                }
            }
        };
        if let Some(provider) = provider {
            if key_present(provider.api_key_env) {
                let dialect = duet_provider::Dialect::parse(provider.dialect).ok_or_else(|| {
                    anyhow::anyhow!("unsupported dialect in {} preset", provider.name)
                })?;
                let key = std::env::var(provider.api_key_env)?;
                let listing = backends::list_frontier_models(
                    provider.base_url,
                    dialect,
                    Some(&key),
                    Duration::from_secs(5),
                )
                .await;
                let ids = match listing.as_ref() {
                    Ok(value) => backends::agent_model_ids(value),
                    Err(error) => {
                        anyhow::ensure!(
                            error != "HTTP 401" && error != "HTTP 403",
                            "{} rejected {}: check that key before setup",
                            provider.label,
                            provider.api_key_env
                        );
                        eprintln!(
                            "{} model listing unavailable ({error}); a preset or explicit model is required",
                            provider.label
                        );
                        Vec::new()
                    }
                };
                let prior = (cfg.str("frontier.base_url")? == provider.base_url)
                    .then(|| cfg.str("frontier.model").ok())
                    .flatten();
                let preferred = prior
                    .as_deref()
                    .or((!provider.model.is_empty()).then_some(provider.model));
                let model = if ids.is_empty() && options.model.is_none() {
                    anyhow::ensure!(
                        preferred.is_some(),
                        "{} has no fallback model; pass --model after checking the provider's catalog",
                        provider.name
                    );
                    preferred.unwrap().to_owned()
                } else {
                    choose_model(
                        "frontier model",
                        &ids,
                        preferred,
                        options.model.as_deref(),
                        options.yes,
                    )?
                };
                let vision = listing
                    .as_ref()
                    .ok()
                    .and_then(|value| backends::model_vision_capability(value, &model))
                    .unwrap_or(model == provider.model && provider.vision);
                eprintln!(
                    "frontier: {} · {model} · {}",
                    provider.label, provider.base_url
                );
                if duet_provider::catalog::Catalog::bundled()
                    .quote(&model)
                    .is_none()
                    && !(cfg.str("pricing.manual_model").ok().as_deref() == Some(model.as_str())
                        && cfg.str("pricing.manual_base_url").ok().as_deref()
                            == Some(provider.base_url))
                {
                    eprintln!(
                        "price for {model} is not in Duet's bundled catalog; set pricing.manual_model, pricing.manual_base_url and the provider's input/output rates before a run so the dollar budget can be enforced"
                    );
                } else if provider.name != "openrouter" {
                    eprintln!(
                        "pricing note: the catalog estimate may differ from {}'s direct rate; owner-supplied rates can override it",
                        provider.label
                    );
                }
                changes.extend([
                    ("frontier.base_url", Value::String(provider.base_url.into())),
                    ("frontier.model", Value::String(model.clone())),
                    (
                        "frontier.api_key_env",
                        Value::String(provider.api_key_env.into()),
                    ),
                    ("frontier.dialect", Value::String(provider.dialect.into())),
                    ("frontier.vision", Value::Boolean(vision)),
                ]);
                if cfg.str("frontier.base_url")? != provider.base_url
                    && !cfg.str("pricing.manual_model")?.is_empty()
                {
                    changes.push(("pricing.manual_model", Value::String(String::new())));
                }
                if cfg.str("frontier.base_url")? != provider.base_url
                    || cfg.str("frontier.model")? != model
                {
                    changes.push(("pricing.frontier_model", Value::String(String::new())));
                }
            } else if options.provider.is_some() || options.model.is_some() {
                anyhow::bail!(
                    "{} is not set; export the provider's API key before setup",
                    provider.api_key_env
                );
            }
        }
    }

    if let Some(url) = options.local_url.as_deref() {
        let endpoint = duet_provider::endpoint::ApprovedEndpoint::new(
            url,
            &duet_provider::Role::Local {
                allowlist: cfg.list("local.allowlist")?,
                allow_plaintext: cfg.bool("local.allow_plaintext")?,
            },
        )
        .map_err(|e| anyhow::anyhow!(e.message))?;
        let same = cfg.str("local.base_url")? == url;
        let key_env = if same {
            cfg.str("local.api_key_env")?
        } else {
            String::new()
        };
        let key = (!key_env.is_empty())
            .then(|| std::env::var(&key_env).ok())
            .flatten();
        let listing = backends::list_models(&endpoint, key.as_deref(), Duration::from_secs(5))
            .await
            .map_err(anyhow::Error::msg)?;
        let ids = backends::agent_model_ids(&listing);
        let preferred = if same {
            cfg.str("local.model").ok()
        } else {
            None
        };
        let model = choose_model(
            "local model",
            &ids,
            preferred.as_deref(),
            options.local_model.as_deref(),
            options.yes,
        )?;
        changes.extend([
            ("local.base_url", Value::String(url.into())),
            ("local.model", Value::String(model)),
            ("local.api_key_env", Value::String(key_env)),
        ]);
    } else if cfg.origin("local.base_url") != Some(Origin::Default)
        && let Some(model) = options.local_model.as_deref()
    {
        let url = cfg.str("local.base_url")?;
        let endpoint = duet_provider::endpoint::ApprovedEndpoint::new(
            &url,
            &duet_provider::Role::Local {
                allowlist: cfg.list("local.allowlist")?,
                allow_plaintext: cfg.bool("local.allow_plaintext")?,
            },
        )
        .map_err(|e| anyhow::anyhow!(e.message))?;
        let key_env = cfg.str("local.api_key_env")?;
        let key = (!key_env.is_empty())
            .then(|| std::env::var(&key_env).ok())
            .flatten();
        let listing = backends::list_models(&endpoint, key.as_deref(), Duration::from_secs(5))
            .await
            .map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            backends::agent_model_ids(&listing)
                .iter()
                .any(|id| id == model),
            "{model} is not listed by the configured local endpoint {url}"
        );
        changes.push(("local.model", Value::String(model.into())));
    } else if cfg.origin("local.base_url") == Some(Origin::Default) {
        let servers =
            backends::discover_loopback(&bootstrap_ports(), Duration::from_millis(1500)).await;
        let serving: Vec<_> = servers.iter().filter(|s| !s.models.is_empty()).collect();
        if serving.is_empty() {
            eprintln!(
                "local: no model server found on loopback; start Ollama, LM Studio, llama.cpp, vLLM, oMLX, MLX, Jan, GPT4All, KoboldCpp, LocalAI or LiteLLM, then rerun setup"
            );
        } else {
            let choices: Vec<(String, &Server, &str)> = serving
                .iter()
                .flat_map(|server| {
                    server.models.iter().map(|model| {
                        (
                            format!("{model} at {}", server.base_url),
                            *server,
                            model.as_str(),
                        )
                    })
                })
                .filter(|(_, _, model)| {
                    options
                        .local_model
                        .as_deref()
                        .is_none_or(|want| want == *model)
                })
                .collect();
            anyhow::ensure!(
                !choices.is_empty(),
                "the requested local model is not listed by a discovered server"
            );
            let labels: Vec<String> = choices.iter().map(|(label, _, _)| label.clone()).collect();
            let preferred = cfg.str("local.model").ok();
            let index = if let Some(want) = preferred.as_deref()
                && let [only] = choices
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, _, model))| *model == want)
                    .collect::<Vec<_>>()
                    .as_slice()
            {
                only.0
            } else {
                let ranked: Vec<String> = choices
                    .iter()
                    .map(|(_, _, model)| (*model).to_owned())
                    .collect();
                choose_number("local model", &labels, suggested(&ranked), options.yes)?
            };
            let (_, server, model) = &choices[index];
            eprintln!("local: {model} · {}", server.base_url);
            changes.extend([
                ("local.base_url", Value::String(server.base_url.clone())),
                ("local.model", Value::String((*model).to_owned())),
                (
                    "local.api_key_env",
                    Value::String(server.api_key_env.unwrap_or("").into()),
                ),
            ]);
        }
    }
    if changes.is_empty() {
        eprintln!("no settings to change; `duet doctor --online` checks the current configuration");
        return Ok(0);
    }
    eprintln!("\nproposed owner settings:");
    for (name, value) in &changes {
        eprintln!("  {name} = {value}");
    }
    if !options.yes {
        if !io::stdin().is_terminal() {
            eprintln!(
                "run `duet setup --yes` to apply this exact discovery (or pass --provider/--model to select)"
            );
            return Ok(2);
        }
        eprint!("Apply these settings? [y/N] ");
        io::stderr().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            return Ok(2);
        }
    }
    let code = apply_owner(cfg, &changes, true)?;
    if code == 0 {
        eprintln!(
            "setup saved; run `duet doctor --online` to check model listings, context, cache and configured vision"
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
    pub api_key_env: Option<String>,
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

fn set_commands(base_url: &str, model: &str, key_env: Option<&str>) -> String {
    let shell = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    let base = shell(&Value::String(base_url.to_owned()).to_string());
    let model = shell(&Value::String(model.to_owned()).to_string());
    let key_line = key_env.map_or_else(String::new, |name| {
        format!(
            "\n  duet config set local.api_key_env {}",
            shell(&Value::String(name.to_owned()).to_string())
        )
    });
    format!(
        "  duet config set local.base_url {base} --confirm\n  duet config set local.model {model}{key_line}"
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
                set_commands(&server.base_url, &model, server.api_key_env)
            );
            Ok(Some(Bootstrap {
                base_url: server.base_url.clone(),
                model,
                api_key_env: server.api_key_env.map(str::to_owned),
            }))
        }
        Pick::Ambiguous => {
            eprintln!("\nmore than one local model is available; choose one:");
            for s in servers.iter().filter(|s| !s.models.is_empty()) {
                for m in &s.models {
                    eprintln!("{}\n", set_commands(&s.base_url, m, s.api_key_env));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_choice_prefers_a_live_default_and_rejects_a_typo() {
        let ids = vec!["other".to_owned(), "gpt-6.1-sol".to_owned()];
        assert_eq!(
            choose_model("frontier", &ids, Some("gpt-6.1-sol"), None, true).unwrap(),
            "gpt-6.1-sol"
        );
        assert!(choose_model("frontier", &ids, None, Some("gpt-6.1-s0l"), true).is_err());
    }

    #[test]
    fn setup_does_not_guess_an_unknown_model_when_listing_fails() {
        assert!(choose_model("frontier", &[], None, None, true).is_err());
        assert_eq!(
            choose_model("frontier", &[], None, Some("explicit"), true).unwrap(),
            "explicit"
        );
        assert_eq!(local_key_env("jan"), Some("JAN_API_KEY"));
    }
}
