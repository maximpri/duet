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
pub mod preflight;

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
    /// Vendor family of the model that writes the lane's code (Duet lanes: the
    /// frontier model's; external lanes: their vendor's). A judge of the same
    /// family is flagged as judging its own family.
    #[serde(default)]
    pub family: Option<String>,
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
    /// Directories outside the run the sandboxed agent may write (expanded), such
    /// as a dedicated login directory whose tokens the agent refreshes.
    #[serde(default)]
    pub writable: Vec<String>,
    /// Whether a quota wait may probe `upstream` with the lane's API key. Lanes
    /// on a CLI subscription have no key to probe with.
    #[serde(default = "yes")]
    pub probe: bool,
    /// Prerequisites `duet-eval preflight` checks without calling a model.
    #[serde(default)]
    pub preflight: preflight::Requirements,
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

/// The model family behind a recorded run: as recorded, else the built-in
/// lane of the same name (records made before families were kept).
pub fn lane_family(record: &RunRecord) -> Option<String> {
    if let Some(f) = record
        .provenance
        .as_ref()
        .and_then(|p| p.lane_family.clone())
    {
        return Some(f);
    }
    let built_in = load_lanes(None).ok()?;
    built_in
        .into_iter()
        .find(|l| l.name == record.lane)
        .and_then(|l| l.family)
}

pub fn find_lane<'a>(lanes: &'a [Lane], name: &str) -> Result<&'a Lane> {
    lanes
        .iter()
        .find(|l| l.name == name)
        .with_context(|| format!("unknown lane {name}"))
}

struct Vars<'a> {
    workspace: &'a Path,
    /// The operator's home (the harness's `HOME`; agents get a per-run `HOME`).
    operator_home: &'a Path,
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
        .replace("{operator_home}", &v.operator_home.to_string_lossy())
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
    /// Set when the proxy could not read some of the run's outbound traffic
    /// (see [`leakproxy::uninspected`]): `leaks` is then only a lower bound
    /// and reports show the run's leaks as not measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaks_unmeasured: Option<String>,
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
    /// Duet lanes: the terminal state Duet recorded in its run summary
    /// (`None` when it recorded none; see [`apply_terminal_rules`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<crate::ledger::DuetTerminal>,
    /// Set when the lane itself failed the run without a result to grade: Duet
    /// ended without a terminal state (a crash, or killed at the time limit),
    /// or its own outbound gate stopped it. Such a run is valid and counts
    /// against the lane: pass rate 0, no success, judge score 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_failure: Option<String>,
    /// Duet's own cost ledger (Duet lanes that wrote a run summary).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duet_ledger: Option<DuetLedger>,
    /// What produced the run: harness build, lane model, agent version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
}

impl RunRecord {
    /// The hidden-test pass rate statistics use: 0 for a product failure.
    pub fn counted_pass_rate(&self) -> f64 {
        match (&self.product_failure, &self.grade) {
            (None, Some(g)) => g.hidden_pass_rate,
            _ => 0.0,
        }
    }

    /// Whether the run counts as a success: never for a product failure.
    pub fn counted_success(&self) -> bool {
        self.product_failure.is_none() && self.grade.as_ref().is_some_and(|g| g.success)
    }
}

/// Versions behind one run, for the benchmark's reproducibility section.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// Commit of the duet_v2 build that ran: `DUET_EVAL_GIT_COMMIT` when the
    /// runner sets it, else `git rev-parse HEAD` in the working directory.
    pub git_commit: Option<String>,
    /// Whether tracked files differed from that commit.
    pub git_dirty: Option<bool>,
    /// `env`, `git` or `unknown`.
    pub commit_source: String,
    pub harness_version: String,
    pub lane_model: String,
    pub lane_upstream: String,
    /// First line of the lane program's `--version`.
    pub agent_version: Option<String>,
    /// The lane's model family (see [`Lane::family`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane_family: Option<String>,
}

/// The duet_v2 commit this run is attributed to (see [`Provenance::git_commit`]).
pub fn build_commit() -> (Option<String>, Option<bool>, &'static str) {
    if let Some(c) = std::env::var("DUET_EVAL_GIT_COMMIT")
        .ok()
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty())
    {
        let dirty = std::env::var("DUET_EVAL_GIT_DIRTY")
            .ok()
            .map(|v| matches!(v.trim(), "1" | "true" | "yes"));
        return (Some(c), dirty, "env");
    }
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(["-c", "core.fsmonitor=false"])
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    match git(&["rev-parse", "HEAD"]).filter(|c| !c.is_empty()) {
        Some(c) => {
            let dirty =
                git(&["status", "--porcelain", "--untracked-files=no"]).map(|s| !s.is_empty());
            (Some(c), dirty, "git")
        }
        None => (None, None, "unknown"),
    }
}

/// First line of `<program> --version` (no model call), at most 200 characters.
pub async fn program_version(program: &str) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new(program)
            .arg("--version")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.chars().take(200).collect())
}

/// The operator's home directory, for `{operator_home}` in lane definitions.
pub fn operator_home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .context("HOME is not set")
}

/// The duet binary built alongside this harness (never a `duet` found elsewhere).
pub fn duet_bin() -> Result<String> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("duet")))
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .context("the duet binary is not built next to duet-eval (cargo build -p duet-cli)")
}

/// The program a lane launches, with `{duet_bin}` resolved.
pub fn lane_program(lane: &Lane) -> Result<String> {
    let first = lane.argv.first().context("empty lane argv")?;
    Ok(if first == "{duet_bin}" {
        duet_bin()?
    } else {
        first.clone()
    })
}

/// Classifies a run from the provider statuses the proxy saw, in order.
/// Errors that come from the machine running the evaluation (a full disk), not
/// from the agent under test.
pub fn host_failure(error: &str) -> bool {
    [
        "No space left on device",
        "os error 28",
        // A storage device that dropped out (a USB disk).
        "Device not configured",
        "os error 6",
        "Input/output error",
        "os error 5",
    ]
    .iter()
    .any(|m| error.contains(m))
}

/// A Duet failure reason that says its own outbound gate refused a request.
pub fn gate_blocked(reason: &str) -> bool {
    reason.contains("request blocked by")
}

/// Classifies a Duet run by the terminal state Duet recorded (read from
/// `workspace` unless the record already holds it), on top of the
/// infrastructure verdict. The rules, in order:
/// - A host failure (a full disk) stays invalid, also when Duet reported it
///   as its failure reason.
/// - Duet stopped by its own outbound gate is a product failure, whatever the
///   proxy saw (it usually saw no request).
/// - Duet ran but recorded no terminal state (it crashed, or the harness
///   killed it at its time limit) is a product failure: the workspace is not
///   graded as if the run had completed.
/// - Otherwise the infrastructure verdict stands (e.g. every request refused
///   for quota before the agent could decide anything).
///
/// Records of other lanes, and old records whose workspace is gone, are left
/// as they are. Applying the rules twice changes nothing.
pub fn apply_terminal_rules(record: &mut RunRecord, workspace: &Path) {
    if record.lane_kind != LaneKind::Duet {
        return;
    }
    let terminal = match &record.terminal {
        Some(t) => Some(t.clone()),
        None if workspace.is_dir() => crate::ledger::read_terminal(workspace),
        None => return,
    };
    record.terminal = terminal.clone();
    if record
        .invalid
        .as_deref()
        .is_some_and(|i| i.starts_with("host failure"))
    {
        return;
    }
    match terminal {
        Some(t) if t.state == "failed" && host_failure(&t.detail) => {
            record.invalid = Some(format!("host failure: {}", t.detail));
            record.product_failure = None;
        }
        Some(t) if t.state == "failed" && gate_blocked(&t.detail) => {
            record.invalid = None;
            record.product_failure =
                Some(format!("stopped by Duet's outbound gate ({})", t.detail));
        }
        Some(_) => {}
        // Duet never started (the harness could not launch it).
        None if record.exit_code.is_none() && !record.timed_out => {}
        None => {
            record.invalid = None;
            record.product_failure = Some(if record.timed_out {
                "no terminal state: killed at the harness time limit".to_owned()
            } else {
                format!(
                    "no terminal state (exit {})",
                    record.exit_code.map_or("unknown".into(), |c| c.to_string())
                )
            });
        }
    }
}

/// Reads a run's `run.json` and applies the current terminal rules.
pub fn load_record(run_dir: &Path) -> Result<RunRecord> {
    let text = fs::read_to_string(run_dir.join("run.json"))
        .with_context(|| format!("{}", run_dir.display()))?;
    let mut record: RunRecord = serde_json::from_str(&text)
        .with_context(|| format!("parsing {}/run.json", run_dir.display()))?;
    apply_terminal_rules(&mut record, &run_dir.join("workspace"));
    Ok(record)
}

/// Whether infrastructure decided the run, from one status per frontier
/// request ([`leakproxy::outcome_statuses`]: a WebSocket message the server
/// answered counts as 200, a session that answered nothing as 0).
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

/// The project configuration a task's repository carries for Duet: its IP
/// marks (`ip.interface_only`, `ip.sealed`). `None` when the task has none.
pub fn duet_project_config(spec: &crate::task::TaskSpec) -> Option<String> {
    if spec.ip.interface_only.is_empty() && spec.ip.sealed.is_empty() {
        return None;
    }
    let mut ip = toml::Table::new();
    ip.insert(
        "interface_only".into(),
        toml::Value::try_from(&spec.ip.interface_only).ok()?,
    );
    ip.insert(
        "sealed".into(),
        toml::Value::try_from(&spec.ip.sealed).ok()?,
    );
    let mut root = toml::Table::new();
    root.insert("ip".into(), toml::Value::Table(ip));
    toml::to_string(&root).ok()
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
    if cfg.lane.kind == LaneKind::Duet
        && let Some(project) = duet_project_config(spec)
    {
        // The repository's own marks (after the git baseline, so untracked).
        fs::create_dir_all(ws.join(".duet"))?;
        fs::write(ws.join(".duet/config.toml"), project)?;
    }

    let proxy_dir = run_dir.join("proxy");
    let proxy = leakproxy::start(
        "127.0.0.1:0".parse()?,
        &cfg.lane.upstream,
        prepared.manifest.clone(),
        &proxy_dir,
    )
    .await?;
    let proxy_url = proxy.base_url();
    let (git_commit, git_dirty, commit_source) = build_commit();
    let agent_version = match lane_program(cfg.lane) {
        Ok(p) => program_version(&p).await,
        Err(_) => None,
    };
    let provenance = Provenance {
        git_commit,
        git_dirty,
        commit_source: commit_source.to_owned(),
        harness_version: env!("CARGO_PKG_VERSION").to_owned(),
        lane_model: cfg.lane.model.clone(),
        lane_upstream: cfg.lane.upstream.clone(),
        agent_version,
        lane_family: cfg.lane.family.clone(),
    };

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
        leaks_unmeasured: leakproxy::uninspected(&proxy_dir)?,
        frontier_requests: leakproxy::frontier_request_count(&leakproxy::read_requests(
            &proxy_dir,
        )?),
        usage_by_model: BTreeMap::new(),
        unreported_requests: 0,
        frontier_cost_usd: None,
        electricity_usd: 0.0,
        total_cost_usd: None,
        error: None,
        invalid: None,
        rate_limited: false,
        terminal: None,
        product_failure: None,
        duet_ledger: match cfg.lane.kind {
            LaneKind::Duet => crate::ledger::read(&ws),
            LaneKind::External => None,
        },
        provenance: Some(provenance),
    };
    let statuses = leakproxy::outcome_statuses(&proxy_dir)?;
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
    // The host, not the agent, decided the outcome: exclude and retry the run.
    if record.invalid.is_none()
        && let Some(e) = record.error.as_deref().filter(|e| host_failure(e))
    {
        record.invalid = Some(format!("host failure: {e}"));
    }
    apply_terminal_rules(&mut record, &ws);
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
    let duet_bin = duet_bin()?;
    let home = operator_home()?;
    let vars = Vars {
        workspace: ws,
        operator_home: &home,
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
        // Seatbelt matches resolved paths, so the directories are canonicalized.
        let extra = lane
            .writable
            .iter()
            .map(|w| {
                let dir = PathBuf::from(expand(w, &vars));
                dir.canonicalize().with_context(|| {
                    format!(
                        "lane {}: writable directory {} is missing (see duet-eval preflight)",
                        lane.name,
                        dir.display()
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut writable = vec![run_dir];
        writable.extend(extra.iter().map(PathBuf::as_path));
        let profile = sandbox_profile(&writable, &lane.allow_hosts);
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

pub fn which(program: &str) -> Option<PathBuf> {
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
            "duet-hybrid-nolocal",
            "duet-hybrid-compact",
            "duet-hybrid-explore",
            "duet-local-only",
            "pi-glm",
            "claude-code",
            "codex",
        ] {
            assert!(find_lane(&lanes, required).is_ok(), "{required}");
        }
    }

    #[test]
    fn built_in_lanes_declare_their_model_family() {
        let lanes = load_lanes(None).unwrap();
        for lane in &lanes {
            assert!(lane.family.is_some(), "{} has no family", lane.name);
        }
        for (name, family) in [
            ("duet-passthrough", "zhipu"),
            ("duet-hybrid", "zhipu"),
            ("duet-hybrid-nolocal", "zhipu"),
            ("duet-hybrid-compact", "zhipu"),
            ("pi-glm", "zhipu"),
            ("claude-code", "anthropic"),
            ("codex", "openai"),
        ] {
            let lane = find_lane(&lanes, name).unwrap();
            assert_eq!(lane.family.as_deref(), Some(family), "{name}");
        }
        // Records made before families were kept fall back to the built-in lane.
        let record: RunRecord = serde_json::from_value(serde_json::json!({
            "task": "S1", "lane": "claude-code", "lane_kind": "external", "seed": 1,
            "run_id": "S1-claude-code-s1", "exit_code": 0, "timed_out": false,
            "wall_seconds": 1.0, "grade": null, "leaks": [], "frontier_requests": 0,
            "usage_by_model": {}, "unreported_requests": 0, "frontier_cost_usd": null,
            "electricity_usd": 0.0, "total_cost_usd": null, "error": null
        }))
        .unwrap();
        assert_eq!(lane_family(&record).as_deref(), Some("anthropic"));
    }

    #[test]
    fn duet_lanes_opt_in_explicitly_to_weaker_settings() {
        for lane in load_lanes(None).unwrap() {
            if lane.kind != LaneKind::Duet {
                continue;
            }
            let passthrough = lane.argv.windows(2).any(|w| w == ["--mode", "passthrough"]);
            assert_eq!(
                passthrough,
                lane.argv.iter().any(|a| a == "--no-privacy"),
                "{}: passthrough needs --no-privacy and nothing else may pass it",
                lane.name
            );
            // A lane whose owner config reaches a remote local model over plain
            // HTTP must opt in, or duet refuses the endpoint.
            for f in &lane.files {
                let Ok(t) = toml::from_str::<toml::Table>(&f.content) else {
                    continue;
                };
                let Some(local) = t.get("local").and_then(toml::Value::as_table) else {
                    continue;
                };
                let url = local
                    .get("base_url")
                    .and_then(toml::Value::as_str)
                    .unwrap_or("");
                let loopback = ["http://127.", "http://localhost", "http://[::1]"]
                    .iter()
                    .any(|p| url.starts_with(p));
                if url.starts_with("http://") && !loopback {
                    assert_eq!(
                        local.get("allow_plaintext").and_then(toml::Value::as_bool),
                        Some(true),
                        "{}",
                        lane.name
                    );
                }
            }
        }
    }

    #[test]
    fn duet_lane_configs_are_valid_duet_owner_configs() {
        // Loaded by Duet's own configuration loader: an unknown or misspelled
        // key, or an invalid value, fails here instead of in every run.
        let d = tempfile::tempdir().unwrap();
        for lane in load_lanes(None).unwrap() {
            for f in lane.files.iter().filter(|f| f.path.contains("duet-config")) {
                let owner = d.path().join(format!("{}.toml", lane.name));
                fs::write(&owner, &f.content).unwrap();
                duet_config::Config::load(&owner, None)
                    .unwrap_or_else(|e| panic!("{}: {e}", lane.name));
            }
        }
    }

    #[test]
    fn the_nolocal_lane_is_hybrid_with_the_local_model_off() {
        let lanes = load_lanes(None).unwrap();
        let lane = find_lane(&lanes, "duet-hybrid-nolocal").unwrap();
        let hybrid = find_lane(&lanes, "duet-hybrid").unwrap();
        assert_eq!(lane.kind, LaneKind::Duet);
        // The same run as duet-hybrid, frontier and objective alike.
        assert_eq!(lane.argv, hybrid.argv);
        assert_eq!(
            (&lane.model, &lane.upstream),
            (&hybrid.model, &hybrid.upstream)
        );
        // No LAN model host, no local key, no electricity charged.
        assert!(!lane.uses_local_model());
        assert!(lane.allow_hosts.is_empty());
        assert_eq!(lane.env_passthrough, ["ZAI_API_KEY"]);
        let [config] = lane.files.as_slice() else {
            panic!("one owner config expected");
        };
        let owner: toml::Table = toml::from_str(&config.content).unwrap();
        let local = owner["local"].as_table().unwrap();
        assert_eq!(
            local.get("enabled").and_then(toml::Value::as_bool),
            Some(false)
        );
        assert_eq!(local.len(), 1, "no local endpoint is configured: {local:?}");
    }

    #[test]
    fn the_explore_lane_is_hybrid_with_the_explorer_on() {
        let lanes = load_lanes(None).unwrap();
        let lane = find_lane(&lanes, "duet-hybrid-explore").unwrap();
        let hybrid = find_lane(&lanes, "duet-hybrid").unwrap();
        // The same run as duet-hybrid: frontier, objective and local model.
        assert_eq!(lane.argv, hybrid.argv);
        assert_eq!(
            (&lane.model, &lane.upstream, &lane.allow_hosts),
            (&hybrid.model, &hybrid.upstream, &hybrid.allow_hosts)
        );
        let owner = |l: &Lane| -> toml::Table { toml::from_str(&l.files[0].content).unwrap() };
        let (mine, theirs) = (owner(lane), owner(hybrid));
        assert_eq!(mine["local"], theirs["local"]);
        assert_eq!(mine["explore"]["enabled"].as_bool(), Some(true));
        assert!(theirs.get("explore").is_none());
    }

    #[test]
    fn duet_lanes_keep_operator_approval_off() {
        // Eval runs have no terminal: with `oversight.approve` on, duet refuses
        // to start. Each Duet lane therefore uses its own owner config (never the
        // operator's, which may turn approval on) and never sets it.
        for lane in load_lanes(None).unwrap() {
            if lane.kind != LaneKind::Duet {
                continue;
            }
            assert_eq!(
                lane.env.get("DUET_CONFIG_HOME").map(String::as_str),
                Some("{run_dir}/duet-config"),
                "{}",
                lane.name
            );
            for f in &lane.files {
                let t: toml::Table = toml::from_str(&f.content).unwrap();
                let approve = t
                    .get("oversight")
                    .and_then(|o| o.get("approve"))
                    .and_then(toml::Value::as_str);
                assert!(matches!(approve, None | Some("off")), "{}", lane.name);
            }
        }
    }

    #[test]
    fn ip_marks_become_duet_project_config() {
        let dir = tempfile::tempdir().unwrap();
        crate::task::tests_support::fixture(dir.path());
        let mut pkg = crate::task::TaskPackage::load(dir.path()).unwrap();
        assert!(duet_project_config(&pkg.spec).is_none());
        pkg.spec.ip.interface_only = vec!["src/pricing/**".into()];
        let text = duet_project_config(&pkg.spec).unwrap();
        let parsed: toml::Table = toml::from_str(&text).unwrap();
        assert_eq!(
            parsed["ip"]["interface_only"].as_array().unwrap()[0].as_str(),
            Some("src/pricing/**")
        );
        assert!(parsed["ip"]["sealed"].as_array().unwrap().is_empty());
    }

    #[test]
    fn records_without_provenance_still_load() {
        let old = serde_json::json!({
            "task": "S1", "lane": "a", "lane_kind": "duet", "seed": 1, "run_id": "S1-a-s1",
            "exit_code": 0, "timed_out": false, "wall_seconds": 1.0, "grade": null, "leaks": [],
            "frontier_requests": 1, "usage_by_model": {}, "unreported_requests": 0,
            "frontier_cost_usd": 0.0, "electricity_usd": 0.0, "total_cost_usd": 0.0, "error": null
        });
        let r: RunRecord = serde_json::from_value(old).unwrap();
        assert!(r.provenance.is_none());
        let (commit, _, source) = build_commit();
        assert_eq!(commit.is_some(), source != "unknown");
    }

    #[tokio::test]
    async fn version_of_a_missing_program_is_none() {
        assert!(
            program_version("/nonexistent/duet-eval-probe")
                .await
                .is_none()
        );
    }

    #[test]
    fn expands_placeholders() {
        let v = Vars {
            workspace: Path::new("/w"),
            operator_home: Path::new("/home/op"),
            run_dir: Path::new("/r"),
            proxy_url: "http://127.0.0.1:9",
            objective: "fix it",
            model: "m",
            duet_bin: "/bin/duet",
        };
        assert_eq!(
            expand(
                "{run_dir}/x {proxy_url} {model} {objective} {operator_home}/.e",
                &v
            ),
            "/r/x http://127.0.0.1:9 m fix it /home/op/.e"
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
        assert!(crate::lanes::host_failure(
            "grading failed: staging ws/.env: No space left on device (os error 28)"
        ));
        assert!(!crate::lanes::host_failure(
            "grading failed: hidden tests timed out"
        ));
    }
}

/// Blocks until the lane's provider accepts a minimal request again (after a
/// quota or rate limit), probing every five minutes, for at most `max_wait`.
pub async fn wait_until_available(lane: &Lane, max_wait: Duration) -> bool {
    if !lane.probe {
        eprintln!(
            "lane {} cannot be probed (subscription); re-run the batch later",
            lane.name
        );
        return false;
    }
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
