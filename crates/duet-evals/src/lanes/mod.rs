// SPDX-License-Identifier: GPL-3.0-or-later
//! Lane definitions and the runner for one (task, seed, lane) combination.
//!
//! This module is the only place in the workspace allowed to name other coding
//! agents: they are launched as black boxes for measurement. Every lane runs in
//! a sandbox whose outbound network is limited to loopback (plus any host the
//! lane explicitly allows, such as a LAN model server), so all frontier traffic
//! must pass through the leak proxy.

use crate::canary::Manifest;
use crate::cost::{PriceTable, Usage, usage_from_proxy_log};
use crate::grade::{GradeReport, grade};
use crate::leakproxy::{self, LeakRecord};
use crate::ledger::DuetLedger;
use crate::task::TaskPackage;
use crate::workspace;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

pub mod judge_cli;

pub const LANES_TOML: &str = include_str!("lanes.toml");

/// Words that could reveal which agent or model produced a change; the judge
/// replaces them before scoring.
pub const IDENTIFYING_NAMES: &[&str] = &[
    "duet",
    "claude",
    "anthropic",
    "codex",
    "openai",
    "gpt",
    "glm",
    "zhipu",
    "z.ai",
    "qwen",
    "omlx",
    "ollama",
    "gemini",
    "pi-agent",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LaneKind {
    Duet,
    External,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LaneFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lane {
    pub name: String,
    pub kind: LaneKind,
    pub upstream: String,
    pub model: String,
    pub argv: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub env_passthrough: Vec<String>,
    #[serde(default)]
    pub files: Vec<LaneFile>,
    /// Non-loopback `host:port` endpoints the sandbox may reach directly.
    #[serde(default)]
    pub allow_hosts: Vec<String>,
    /// Wrap the lane in the harness sandbox. Duet lanes set this to false: macOS
    /// cannot nest sandboxes, and Duet sandboxes its own commands.
    #[serde(default = "yes")]
    pub sandbox: bool,
}

fn yes() -> bool {
    true
}

impl Lane {
    /// Lanes that reach a local model server (listed in `allow_hosts`).
    pub fn uses_local_model(&self) -> bool {
        !self.allow_hosts.is_empty()
    }
}

#[derive(Debug, Deserialize)]
struct LaneTable {
    lane: Vec<Lane>,
}

pub fn load_lanes(override_path: Option<&Path>) -> Result<Vec<Lane>> {
    let text = match override_path {
        Some(p) => fs::read_to_string(p)?,
        None => LANES_TOML.to_owned(),
    };
    let table: LaneTable = toml::from_str(&text).context("parsing lanes")?;
    Ok(table.lane)
}

pub fn find_lane<'a>(lanes: &'a [Lane], name: &str) -> Result<&'a Lane> {
    lanes
        .iter()
        .find(|l| l.name == name)
        .with_context(|| format!("unknown lane {name}"))
}

struct Vars<'a> {
    workspace: &'a Path,
    run_dir: &'a Path,
    proxy_url: &'a str,
    objective: &'a str,
    model: &'a str,
    duet_bin: &'a str,
}

fn expand(template: &str, v: &Vars<'_>) -> String {
    template
        .replace("{workspace}", &v.workspace.to_string_lossy())
        .replace("{run_dir}", &v.run_dir.to_string_lossy())
        .replace("{proxy_url}", v.proxy_url)
        .replace("{objective}", v.objective)
        .replace("{model}", v.model)
        .replace("{duet_bin}", v.duet_bin)
}

/// Seatbelt profile: everything allowed except writes outside the run and
/// temporary directories, and outbound network other than loopback and the
/// lane's explicitly allowed hosts.
pub fn sandbox_profile(writable: &[&Path], allow_hosts: &[String]) -> String {
    let mut p = String::from("(version 1)\n(allow default)\n(deny file-write*)\n");
    for dir in writable {
        p.push_str(&format!(
            "(allow file-write* (subpath \"{}\"))\n",
            dir.to_string_lossy()
        ));
    }
    p.push_str(
        "(allow file-write* (subpath \"/private/tmp\") (subpath \"/private/var/folders\") \
         (literal \"/dev/null\") (literal \"/dev/tty\") (regex #\"^/dev/fd/\"))\n",
    );
    p.push_str("(deny network-outbound)\n(allow network-outbound (remote ip \"localhost:*\"))\n");
    p.push_str("(allow network-outbound (remote unix-socket))\n");
    for host in allow_hosts {
        // Seatbelt accepts only "localhost" or "*" as the host part, so a LAN
        // host is admitted by port; the proxy remains the only route to the
        // frontier because frontier providers are reached over 443.
        let port = host.rsplit(':').next().unwrap_or("*");
        p.push_str(&format!(
            "(allow network-outbound (remote ip \"*:{port}\"))\n"
        ));
    }
    p
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub task: String,
    pub lane: String,
    pub lane_kind: LaneKind,
    pub seed: u64,
    pub run_id: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub wall_seconds: f64,
    pub grade: Option<GradeReport>,
    pub leaks: Vec<LeakRecord>,
    pub frontier_requests: usize,
    pub usage_by_model: BTreeMap<String, Usage>,
    pub unreported_requests: u64,
    /// Frontier dollars at list price; `None` if any model is unpriced.
    pub frontier_cost_usd: Option<f64>,
    /// Local model electricity. Until Duet reports model busy time, wall-clock
    /// time is used as a conservative upper bound.
    pub electricity_usd: f64,
    /// Frontier plus electricity; `None` if any frontier model is unpriced.
    pub total_cost_usd: Option<f64>,
    pub error: Option<String>,
    /// Set when infrastructure (not the agent) decided the outcome, e.g. the provider
    /// refused every request. Invalid runs are excluded from every statistic.
    #[serde(default)]
    pub invalid: Option<String>,
    /// Whether the provider rate-limited or quota-limited the run.
    #[serde(default)]
    pub rate_limited: bool,
    /// Duet's own cost ledger (Duet lanes that wrote a run summary).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duet_ledger: Option<DuetLedger>,
}

/// Classifies a run from the provider statuses the proxy saw, in order.
pub fn infra_verdict(statuses: &[u16]) -> (Option<String>, bool) {
    let limited = statuses.contains(&429);
    let ok = statuses.iter().filter(|s| (200..300).contains(*s)).count();
    let invalid = match statuses.last() {
        None => Some("the agent made no frontier request".to_owned()),
        Some(_) if ok == 0 => Some(format!(
            "no frontier request succeeded (statuses {statuses:?})"
        )),
        Some(last) if !(200..300).contains(last) => {
            Some(format!("the final frontier request failed with {last}"))
        }
        _ => None,
    };
    (invalid, limited)
}

pub struct RunConfig<'a> {
    pub package: &'a TaskPackage,
    pub lane: &'a Lane,
    pub seed: u64,
    pub out_dir: &'a Path,
    pub prices: &'a PriceTable,
    pub sandbox: bool,
    /// Average draw of the local model host while generating.
    pub local_watts: f64,
}

pub async fn run_one(cfg: RunConfig<'_>) -> Result<RunRecord> {
    let spec = &cfg.package.spec;
    cfg.package.verify_seal()?;
    let run_id = format!("{}-{}-s{}", spec.id, cfg.lane.name, cfg.seed);
    let run_dir = cfg.out_dir.join(&run_id);
    ensure!(
        !run_dir.exists(),
        "run directory {} already exists",
        run_dir.display()
    );
    fs::create_dir_all(&run_dir)?;
    let run_dir = run_dir.canonicalize()?;
    let ws = run_dir.join("workspace");
    let prepared = workspace::prepare(cfg.package, cfg.seed, &run_id, &ws)?;
    // The manifest stays outside the workspace, where agents cannot read it.
    fs::write(
        run_dir.join("manifest.json"),
        serde_json::to_string_pretty(&prepared.manifest)?,
    )?;
    fs::write(run_dir.join("objective.md"), &cfg.package.objective)?;

    let proxy_dir = run_dir.join("proxy");
    let proxy = leakproxy::start(
        "127.0.0.1:0".parse()?,
        &cfg.lane.upstream,
        prepared.manifest.clone(),
        &proxy_dir,
    )
    .await?;
    let proxy_url = proxy.base_url();

    let outcome = launch(&cfg, &run_dir, &ws, &proxy_url).await;
    proxy.stop();

    let mut record = RunRecord {
        task: spec.id.clone(),
        lane: cfg.lane.name.clone(),
        lane_kind: cfg.lane.kind,
        seed: cfg.seed,
        run_id: run_id.clone(),
        exit_code: None,
        timed_out: false,
        wall_seconds: 0.0,
        grade: None,
        leaks: leakproxy::read_leaks(&proxy_dir)?,
        frontier_requests: leakproxy::read_requests(&proxy_dir)?.len(),
        usage_by_model: BTreeMap::new(),
        unreported_requests: 0,
        frontier_cost_usd: None,
        electricity_usd: 0.0,
        total_cost_usd: None,
        error: None,
        invalid: None,
        rate_limited: false,
        duet_ledger: match cfg.lane.kind {
            LaneKind::Duet => crate::ledger::read(&ws),
            LaneKind::External => None,
        },
    };
    let statuses: Vec<u16> = leakproxy::read_requests(&proxy_dir)?
        .iter()
        .map(|r| r.status)
        .collect();
    let (invalid, limited) = infra_verdict(&statuses);
    record.invalid = invalid;
    record.rate_limited = limited;
    match outcome {
        Ok(o) => {
            record.exit_code = o.exit_code;
            record.timed_out = o.timed_out;
            record.wall_seconds = o.wall_seconds;
        }
        Err(e) => record.error = Some(format!("{e:#}")),
    }

    let usage = usage_from_proxy_log(&proxy_dir)?;
    record.unreported_requests = usage.unreported_requests;
    let mut total = Some(0.0);
    for (model, u) in &usage.by_model {
        total = match (total, cfg.prices.cost(model, *u)) {
            (Some(t), Ok(c)) => Some(t + c),
            _ => None,
        };
    }
    record.frontier_cost_usd = total;
    if cfg.lane.uses_local_model() {
        record.electricity_usd = cfg.prices.electricity(cfg.local_watts, record.wall_seconds);
    }
    record.total_cost_usd = total.map(|t| t + record.electricity_usd);
    record.usage_by_model = usage.by_model;

    let timeout = Duration::from_secs(15 * 60);
    match grade(
        cfg.package,
        &ws,
        &prepared.manifest,
        &run_dir.join("grade"),
        timeout,
    ) {
        Ok(g) => record.grade = Some(g),
        Err(e) => {
            let msg = format!("grading failed: {e:#}");
            record.error = Some(record.error.map_or(msg.clone(), |x| format!("{x}; {msg}")));
        }
    }
    fs::write(
        run_dir.join("run.json"),
        serde_json::to_string_pretty(&record)?,
    )?;
    Ok(record)
}

struct LaunchOutcome {
    exit_code: Option<i32>,
    timed_out: bool,
    wall_seconds: f64,
}

async fn launch(
    cfg: &RunConfig<'_>,
    run_dir: &Path,
    ws: &Path,
    proxy_url: &str,
) -> Result<LaunchOutcome> {
    let lane = cfg.lane;
    // The duet binary built alongside this harness (never a `duet` found elsewhere).
    let duet_bin = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("duet")))
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .context("the duet binary is not built next to duet-eval (cargo build -p duet-cli)")?;
    let vars = Vars {
        workspace: ws,
        run_dir,
        proxy_url,
        objective: &cfg.package.objective,
        model: &lane.model,
        duet_bin: &duet_bin,
    };
    for f in &lane.files {
        let path = PathBuf::from(expand(&f.path, &vars));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, expand(&f.content, &vars))?;
    }
    let argv: Vec<String> = lane.argv.iter().map(|a| expand(a, &vars)).collect();
    let (program, args) = argv.split_first().context("empty lane argv")?;
    if which(program).is_none() {
        bail!("lane {}: program {program} is not installed", lane.name);
    }

    let mut command = if cfg.sandbox && lane.sandbox {
        let profile = sandbox_profile(&[run_dir], &lane.allow_hosts);
        let profile_path = run_dir.join("sandbox.sb");
        fs::write(&profile_path, profile)?;
        let mut c = tokio::process::Command::new("/usr/bin/sandbox-exec");
        c.arg("-f").arg(&profile_path).arg(program).args(args);
        c
    } else {
        let mut c = tokio::process::Command::new(program);
        c.args(args);
        c
    };
    command
        .current_dir(ws)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", run_dir.join("home"))
        .env("TMPDIR", run_dir.join("tmp"))
        .env("TERM", "dumb")
        .env("NO_COLOR", "1");
    fs::create_dir_all(run_dir.join("home"))?;
    fs::create_dir_all(run_dir.join("tmp"))?;
    for key in &lane.env_passthrough {
        if let Some(v) = std::env::var_os(key) {
            command.env(key, v);
        }
    }
    for (k, v) in &lane.env {
        command.env(k, expand(v, &vars));
    }
    command
        .stdin(Stdio::null())
        .stdout(fs::File::create(run_dir.join("agent.stdout"))?)
        .stderr(fs::File::create(run_dir.join("agent.stderr"))?)
        .kill_on_drop(true);

    let budget = Duration::from_secs(u64::from(cfg.package.spec.time_budget_minutes) * 60);
    let started = Instant::now();
    let mut child = command
        .spawn()
        .with_context(|| format!("starting lane {}", lane.name))?;
    let (exit_code, timed_out) = match tokio::time::timeout(budget, child.wait()).await {
        Ok(status) => (status?.code(), false),
        Err(_) => {
            let _ = child.kill().await;
            (None, true)
        }
    };
    Ok(LaunchOutcome {
        exit_code,
        timed_out,
        wall_seconds: started.elapsed().as_secs_f64(),
    })
}

fn which(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        return Some(PathBuf::from(program)).filter(|p| p.exists());
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(program))
            .find(|p| p.is_file())
    })
}

pub fn manifest_of(run_dir: &Path) -> Result<Manifest> {
    Ok(serde_json::from_str(&fs::read_to_string(
        run_dir.join("manifest.json"),
    )?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_lanes_parse_and_have_unique_names() {
        let lanes = load_lanes(None).unwrap();
        let mut names: Vec<_> = lanes.iter().map(|l| l.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), lanes.len());
        for required in [
            "duet-passthrough",
            "duet-hybrid",
            "duet-local-only",
            "pi-glm",
        ] {
            assert!(find_lane(&lanes, required).is_ok(), "{required}");
        }
    }

    #[test]
    fn expands_placeholders() {
        let v = Vars {
            workspace: Path::new("/w"),
            run_dir: Path::new("/r"),
            proxy_url: "http://127.0.0.1:9",
            objective: "fix it",
            model: "m",
            duet_bin: "/bin/duet",
        };
        assert_eq!(
            expand("{run_dir}/x {proxy_url} {model} {objective}", &v),
            "/r/x http://127.0.0.1:9 m fix it"
        );
    }

    #[test]
    fn sandbox_limits_writes_and_network() {
        let p = sandbox_profile(&[Path::new("/runs/a")], &["192.168.50.132:8080".into()]);
        assert!(p.contains("(deny file-write*)"));
        assert!(p.contains("(subpath \"/runs/a\")"));
        assert!(p.contains("(deny network-outbound)"));
        assert!(p.contains("localhost:*"));
        assert!(p.contains("*:8080"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sandbox_blocks_outside_writes_and_direct_network() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().canonicalize().unwrap();
        let profile = run.join("p.sb");
        fs::write(&profile, sandbox_profile(&[&run], &[])).unwrap();
        let inside = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-f")
            .arg(&profile)
            .args(["/bin/sh", "-c", &format!("echo x > {}/ok", run.display())])
            .status()
            .unwrap();
        assert!(inside.success());
        let home = std::env::var("HOME").unwrap();
        let outside = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-f")
            .arg(&profile)
            .args([
                "/bin/sh",
                "-c",
                &format!("echo x > {home}/.duet-eval-sandbox-probe"),
            ])
            .status()
            .unwrap();
        assert!(!outside.success());
        let net = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-f")
            .arg(&profile)
            .args([
                "/usr/bin/curl",
                "-s",
                "-m",
                "5",
                "-o",
                "/dev/null",
                "https://example.com",
            ])
            .status()
            .unwrap();
        assert!(!net.success(), "direct outbound network must be blocked");
    }
}

#[cfg(test)]
mod infra_tests {
    use super::infra_verdict;

    #[test]
    fn classifies_infrastructure_outcomes() {
        assert_eq!(
            infra_verdict(&[200, 200, 429, 200]).0,
            None,
            "recovered rate limit is valid"
        );
        assert!(infra_verdict(&[200, 200, 429, 200]).1);
        assert!(infra_verdict(&[429, 429, 429]).0.is_some());
        assert!(
            infra_verdict(&[200, 200, 429, 429]).0.is_some(),
            "final request failed"
        );
        assert!(infra_verdict(&[]).0.is_some());
        assert!(infra_verdict(&[200, 503, 200]).0.is_none());
    }
}

/// Blocks until the lane's provider accepts a minimal request again (after a
/// quota or rate limit), probing every five minutes, for at most `max_wait`.
pub async fn wait_until_available(lane: &Lane, max_wait: Duration) -> bool {
    let started = Instant::now();
    let key = lane
        .env_passthrough
        .iter()
        .find_map(|k| std::env::var(k).ok());
    let client = reqwest::Client::new();
    loop {
        let mut req = client
            .post(format!(
                "{}/chat/completions",
                lane.upstream.trim_end_matches('/')
            ))
            .json(&serde_json::json!({"model": lane.model, "max_tokens": 1,
                                      "messages": [{"role": "user", "content": "ping"}]}));
        if let Some(k) = &key {
            req = req.bearer_auth(k);
        }
        if let Ok(resp) = req.send().await
            && resp.status().is_success()
        {
            return true;
        }
        if started.elapsed() >= max_wait {
            return false;
        }
        eprintln!(
            "provider for lane {} is unavailable; waiting 5 minutes",
            lane.name
        );
        tokio::time::sleep(Duration::from_secs(300)).await;
    }
}
