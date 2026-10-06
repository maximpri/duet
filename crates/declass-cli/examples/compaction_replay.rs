// SPDX-License-Identifier: GPL-3.0-or-later
//! Replays a recorded run's conversation through context compaction offline,
//! with the live local model, and reports what compaction would have done:
//! how often it triggers, tokens before and after each event, local seconds,
//! and the request tokens over the run with and without it. The frontier's
//! turns after an event are assumed unchanged (they resend the compacted
//! context); in a real run the frontier would re-read some of what was
//! condensed.
//!
//! ```text
//! cargo run -p declass-cli --example compaction_replay -- <run-dir>
//!     [--at 100000] [--to 0.4] [--window 200000] [--mask-at 0.7] [--out <dir>]
//!     [--dry <summary chars>]
//! ```
//!
//! `--dry` calls no model: every summary is that many characters of filler,
//! for how often compaction triggers and what it saves at a given size.
//!
//! Both sides of the comparison are simulated from the recorded items:
//! "masking" is today's loop (masking at `--mask-at` of `--window`),
//! "compaction" adds `context.compaction` at `--at`/`--to` in front of it.
//! Tokens are Declass's own estimate (the one both mechanisms decide by);
//! "uncached" counts, per request, what follows the prefix it shares with
//! the request before it (what a provider's prefix cache cannot serve).
//!
//! The run directory is copied to a temporary directory first (the engine
//! opens its vault there), so nothing in it changes. The local model is the
//! owner's (`local.*` in the owner configuration; its key only by the
//! environment variable `local.api_key_env` names). `--out` receives each
//! summary and the report.

use declass_agent::Compaction;
use declass_agent::compaction::{self, Event, State};
use declass_agent::context::{estimate, mask_if_needed};
use declass_agent::transcript::{Entry, Transcript};
use declass_boundary::engine::Engine;
use declass_boundary::local::LocalReader;
use declass_boundary::model::{Item, Request};
use declass_boundary::policy::Policy;
use declass_boundary::view::{Condensed, Presenter, Source};
use declass_config::Config;
use declass_provider::{ChatProvider, ProviderConfig, Role};
use std::path::{Path, PathBuf};

struct Args {
    run_dir: PathBuf,
    at: u64,
    to: f64,
    window: u64,
    mask_at: f64,
    out: Option<PathBuf>,
    dry: Option<usize>,
}

fn args() -> Result<Args, String> {
    let mut a = std::env::args().skip(1);
    let mut out = Args {
        run_dir: PathBuf::new(),
        at: 100_000,
        to: 0.4,
        window: 200_000,
        mask_at: 0.7,
        out: None,
        dry: None,
    };
    let mut positional = None;
    while let Some(x) = a.next() {
        let mut value = |name: &str| a.next().ok_or(format!("{name} needs a value"));
        match x.as_str() {
            "--at" => out.at = value("--at")?.parse().map_err(|e| format!("--at: {e}"))?,
            "--to" => out.to = value("--to")?.parse().map_err(|e| format!("--to: {e}"))?,
            "--window" => {
                out.window = value("--window")?
                    .parse()
                    .map_err(|e| format!("--window: {e}"))?
            }
            "--mask-at" => {
                out.mask_at = value("--mask-at")?
                    .parse()
                    .map_err(|e| format!("--mask-at: {e}"))?
            }
            "--out" => out.out = Some(PathBuf::from(value("--out")?)),
            "--dry" => out.dry = Some(value("--dry")?.parse().map_err(|e| format!("--dry: {e}"))?),
            _ if positional.is_none() => positional = Some(PathBuf::from(x)),
            _ => return Err(format!("unexpected argument {x}")),
        }
    }
    out.run_dir = positional.ok_or("usage: compaction_replay <run-dir> [--at N] [--to F] [--window N] [--mask-at F] [--out DIR]")?;
    Ok(out)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), target)?;
        }
    }
    Ok(())
}

/// The run's system prompt, from the first request in its audit log
/// (`<workspace>/.declass/audit/<run-id>.jsonl`), if there is one.
fn recorded_system(run_dir: &Path) -> Option<String> {
    let id = run_dir.file_name()?.to_str()?;
    let log = run_dir
        .parent()?
        .parent()?
        .join("audit")
        .join(format!("{id}.jsonl"));
    let text = std::fs::read_to_string(log).ok()?;
    text.lines().find_map(|l| {
        let v: serde_json::Value = serde_json::from_str(l).ok()?;
        let m = v.get("request")?.get("messages")?.get(0)?;
        (m["role"] == "system").then(|| m["content"].as_str().map(str::to_owned))?
    })
}

fn policy(cfg: &Config) -> Result<Policy, String> {
    let e = |e: declass_config::ConfigError| e.to_string();
    Ok(Policy {
        sensitive_globs: cfg.list("sensitivity.globs").map_err(e)?,
        protected_paths: cfg.list("sensitivity.protected_paths").map_err(e)?,
        command_output_sensitive: cfg
            .bool("sensitivity.command_output_sensitive")
            .map_err(e)?,
        raw_ok_commands: cfg.list("sensitivity.raw_ok_commands").map_err(e)?,
        secret_sinks: cfg.list("sensitivity.secret_sinks").map_err(e)?,
        detect_secrets: cfg.bool("sensitivity.detect_secrets").map_err(e)?,
        detect_pii: cfg.bool("sensitivity.detect_pii").map_err(e)?,
        detect_entropy: cfg.bool("sensitivity.detect_entropy").map_err(e)?,
        custom_patterns: cfg.list("sensitivity.custom_patterns").map_err(e)?,
        bulky_tokens: cfg.int("sensitivity.bulky_tokens").map_err(e)? as usize,
        bulky_file_tokens: cfg.int("sensitivity.bulky_file_tokens").map_err(e)? as usize,
        ..Policy::default()
    })
}

fn local_reader(cfg: &Config) -> Result<LocalReader, String> {
    let e = |e: declass_config::ConfigError| e.to_string();
    let url = cfg.str("local.base_url").map_err(e)?;
    let mut pc = ProviderConfig::new(
        &url,
        &cfg.str("local.model").map_err(e)?,
        Role::Local {
            allowlist: cfg.list("local.allowlist").map_err(e)?,
            allow_plaintext: cfg.bool("local.allow_plaintext").map_err(e)?,
        },
    );
    let key_env = cfg.str("local.api_key_env").map_err(e)?;
    if !key_env.is_empty() {
        pc.api_key_env = Some(key_env);
    }
    Ok(LocalReader::new(
        ChatProvider::with_reqwest(pc).map_err(|e| e.message)?,
    ))
}

/// The engine's summaries (or, dry, filler), with the size of each input.
struct Recording<'a> {
    engine: &'a Engine,
    dry: Option<usize>,
    input: std::sync::Mutex<usize>,
}

impl Presenter for Recording<'_> {
    fn present(&self, source: &Source, bytes: &[u8]) -> String {
        self.engine.present(source, bytes)
    }
    fn can_condense(&self) -> bool {
        true
    }
    fn condense(&self, conversation: &str) -> Option<Condensed> {
        *self.input.lock().unwrap() = conversation.len();
        match self.dry {
            Some(n) => Some(Condensed {
                summary: Ok("notes ".repeat(n / 6)),
                seconds: 0.0,
            }),
            None => self.engine.condense(conversation),
        }
    }
}

/// A simulated conversation and what its requests cost.
#[derive(Default)]
struct Side {
    items: Vec<Item>,
    previous: Vec<Item>,
    requests: u64,
    tokens: u64,
    uncached: u64,
    peak: u64,
    masked_events: u64,
}

impl Side {
    /// One request of the conversation as it now is.
    fn request(&mut self, system: &str) {
        let total = estimate(&self.items, system);
        let shared = self
            .items
            .iter()
            .zip(&self.previous)
            .take_while(|(a, b)| a == b)
            .count();
        let cached = if self.requests == 0 {
            0
        } else {
            estimate(&self.items[..shared], system)
        };
        self.requests += 1;
        self.tokens += total;
        self.uncached += total.saturating_sub(cached);
        self.peak = self.peak.max(total);
        self.previous = self.items.clone();
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    if let Err(e) = replay().await {
        eprintln!("compaction_replay: {e}");
        std::process::exit(1);
    }
}

async fn replay() -> Result<(), String> {
    let a = args()?;
    let c = Compaction { at: a.at, to: a.to };
    let cfg =
        Config::load(&declass_config::owner_config_path(), None).map_err(|e| e.to_string())?;
    let temp = tempfile::tempdir().map_err(|e| e.to_string())?;
    let run_copy = temp.path().join("run");
    copy_dir(&a.run_dir, &run_copy).map_err(|e| format!("copying the run: {e}"))?;
    let reader = match a.dry {
        Some(_) => None,
        None => Some(local_reader(&cfg)?),
    };
    let engine = Engine::open(&run_copy, policy(&cfg)?, reader).map_err(|e| e.to_string())?;
    let presenter = Recording {
        engine: engine.as_ref(),
        dry: a.dry,
        input: std::sync::Mutex::new(0),
    };
    let (filter, _check) = engine.outbound();
    let as_sent = |older: &[Item]| -> Vec<Item> {
        let mut r = Request {
            items: older.to_vec(),
            ..Request::default()
        };
        let _ = filter.apply(&mut r);
        r.items
    };
    let system = recorded_system(&a.run_dir).unwrap_or_else(|| {
        declass_agent::prompt::system_prompt("repository", &[], &declass_agent::tools::specs())
    });
    let entries = Transcript::read(&a.run_dir).map_err(|e| e.to_string())?;
    if let Some(out) = &a.out {
        std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    }

    let mut masking = Side::default();
    let mut compacting = Side::default();
    let mut state = State::default();
    let mut report = String::new();
    let mut reported_prompt = 0u64;
    let mut events = 0;
    let mut local_seconds = 0.0;
    report.push_str(&format!(
        "Run {}: compact_at {} to {}, window {} mask_at {}\n\n",
        a.run_dir.display(),
        a.at,
        a.to,
        a.window,
        a.mask_at
    ));
    report.push_str("| request | event | items | tokens before | tokens after | local s | input chars | local in / out tokens | summary chars |\n|---|---|---|---|---|---|---|---|---|\n");
    for e in entries {
        match e {
            Entry::Item { item } => {
                masking.items.push(item.clone());
                compacting.items.push(item);
            }
            Entry::Usage { usage, .. } => {
                reported_prompt += usage.input + usage.cache_read + usage.cache_write;
                // Today's loop.
                if !mask_if_needed(&mut masking.items, &system, a.window, a.mask_at).is_empty() {
                    masking.masked_events += 1;
                }
                masking.request(&system);
                // With compaction in front of it.
                let n = compacting.requests + 1;
                let _ = engine.take_local_stats();
                let decided =
                    compaction::decide(&compacting.items, &state, &system, c, &as_sent, &presenter);
                let input = std::mem::take(&mut *presenter.input.lock().unwrap());
                let local = engine.take_local_stats();
                if let Some(event) = decided {
                    events += 1;
                    let (kind, items, before, after, seconds, summary) = match &event {
                        Event::Masked {
                            positions,
                            tokens_before,
                            tokens_after,
                        } => (
                            "masked",
                            positions.len(),
                            *tokens_before,
                            *tokens_after,
                            0.0,
                            None,
                        ),
                        Event::Compacted {
                            head,
                            upto,
                            text,
                            tokens_before,
                            tokens_after,
                            local_seconds,
                        } => (
                            "compacted",
                            upto - head,
                            *tokens_before,
                            *tokens_after,
                            *local_seconds,
                            Some(text.clone()),
                        ),
                        Event::Failed {
                            items,
                            tokens,
                            reason,
                            local_seconds,
                            ..
                        } => {
                            eprintln!("request {n}: compaction failed: {reason}");
                            ("failed", *items, *tokens, *tokens, *local_seconds, None)
                        }
                    };
                    local_seconds += seconds;
                    let (lin, lout) = local
                        .as_ref()
                        .map_or((0, 0), |s| (s.input_tokens, s.output_tokens));
                    report.push_str(&format!(
                        "| {n} | {kind} | {items} | {before} | {after} | {seconds:.1} | {input} | {lin} / {lout} | {} |\n",
                        summary.as_ref().map_or(0, |s| s.chars().count())
                    ));
                    eprintln!(
                        "request {n}: {kind} {items} items, {before} -> {after} tokens, {seconds:.1}s local"
                    );
                    if let (Some(out), Some(text)) = (&a.out, &summary) {
                        std::fs::write(
                            out.join(format!("summary-{events:02}-request-{n}.md")),
                            text,
                        )
                        .map_err(|e| e.to_string())?;
                    }
                    compaction::apply(&mut compacting.items, &mut state, &event);
                }
                if !mask_if_needed(&mut compacting.items, &system, a.window, a.mask_at).is_empty() {
                    compacting.masked_events += 1;
                }
                compacting.request(&system);
            }
            _ => {}
        }
    }
    let pct = |x: u64, of: u64| 100.0 * x as f64 / of.max(1) as f64;
    report.push_str(&format!(
        "\n| | requests | request tokens | uncached tokens | peak request | window maskings |\n|---|---|---|---|---|---|\n\
| masking (today) | {} | {} | {} | {} | {} |\n\
| compaction | {} | {} ({:+.1}%) | {} ({:+.1}%) | {} | {} |\n",
        masking.requests,
        masking.tokens,
        masking.uncached,
        masking.peak,
        masking.masked_events,
        compacting.requests,
        compacting.tokens,
        pct(compacting.tokens, masking.tokens) - 100.0,
        compacting.uncached,
        pct(compacting.uncached, masking.uncached) - 100.0,
        compacting.peak,
        compacting.masked_events,
    ));
    report.push_str(&format!(
        "\nEvents: {events}; local seconds in total: {local_seconds:.0}. Prompt tokens the provider reported for the recorded run: {reported_prompt} (the estimate for today's loop: {}).\n",
        masking.tokens
    ));
    for r in [0.1, 0.186] {
        let cost = |s: &Side| (s.uncached as f64 + r * (s.tokens - s.uncached) as f64) / 1e6;
        report.push_str(&format!(
            "Input cost in millions of full-price tokens, cache reads at {r} of the input price: masking {:.2}, compaction {:.2} ({:+.1}%).\n",
            cost(&masking),
            cost(&compacting),
            100.0 * (cost(&compacting) / cost(&masking) - 1.0)
        ));
    }
    println!("{report}");
    if let Some(out) = &a.out {
        std::fs::write(out.join("report.md"), &report).map_err(|e| e.to_string())?;
    }
    Ok(())
}
