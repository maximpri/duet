// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-turn cost profile of a batch, and projections of its recorded tokens
//! at other list prices (M5.2 instrumentation).
//!
//! Frontier cost is turns × context: every request re-sends the conversation.
//! A run's profile keeps, for each frontier request, the context it sent
//! (uncached input + cache reads + cache writes), its output and its cost, from
//! the provider's own usage in the leak proxy's capture — the numbers the run's
//! cost was computed from, so every lane is profiled the same way. What each
//! turn was for comes from Duet's transcript (the tools the turn called), so
//! only Duet lanes have turns by cause.
//!
//! A projection prices every run's recorded token counts at another model's
//! list price. It keeps the recorded counts: another tokenizer would count the
//! same text differently, and another provider's cache would split the input
//! differently (see [`render_projection`] for how that bounds the result).

use crate::cost::{Price, PriceTable, RequestUsage, Usage};
use crate::lanes::RunRecord;
use crate::report::{ALPHA, BOOTSTRAP_ITERS};
use crate::stats::{self, Interval};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

/// The Z.ai, Anthropic and OpenAI flagships `--project-prices flagships`
/// projects at (rows of `pricing.toml`, which cites each price and its date).
pub const FLAGSHIPS: &[&str] = &["glm-5.3", "claude-opus-5-5", "gpt-5.5"];

/// What a frontier turn was for, from the tools it called.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    Reading,
    Editing,
    Commands,
    AskLocal,
    Other,
}

impl Cause {
    pub const ALL: [Cause; 5] = [
        Cause::Reading,
        Cause::Editing,
        Cause::Commands,
        Cause::AskLocal,
        Cause::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Cause::Reading => "reading",
            Cause::Editing => "editing",
            Cause::Commands => "commands",
            Cause::AskLocal => "ask_local",
            Cause::Other => "other",
        }
    }

    /// The cause a call of Duet's tool `name` counts toward. Tools that only
    /// look (files, handles, search, git history, the web, code navigation,
    /// the local explorer) are reading; tools that change the repository are editing; `finish`,
    /// `delegate`, questions to the operator and MCP tools are other.
    pub fn of_tool(name: &str) -> Self {
        match name {
            "ask_local" => Cause::AskLocal,
            "run_command" => Cause::Commands,
            "edit_file" | "write_file" | "edit_protected" | "rename" | "git_commit" => {
                Cause::Editing
            }
            "read_file" | "read_raw" | "list_files" | "search" | "diff" | "code_nav"
            | "web_fetch" | "web_search" | "git_log" | "git_status" | "git_show" | "git_blame"
            | "explore" => Cause::Reading,
            _ => Cause::Other,
        }
    }
}

/// Turns per cause (fractional: see [`causes_from_transcript`]).
pub type Causes = BTreeMap<Cause, f64>;

fn no_causes() -> Causes {
    Cause::ALL.iter().map(|c| (*c, 0.0)).collect()
}

/// The tools an assistant message called, if `entry` is one (a sub-agent's
/// entries are nested under its id).
fn assistant_tools(entry: &Value) -> Option<Vec<&str>> {
    match entry.get("kind")?.as_str()? {
        "subagent" => assistant_tools(entry.get("entry")?),
        "item" => {
            let item = entry.get("item")?;
            (item.get("type")?.as_str()? == "assistant").then(|| {
                item.get("tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c.get("name")?.as_str())
                    .collect()
            })
        }
        _ => None,
    }
}

/// Turns by cause in a Duet transcript, sub-agents' turns included. Each
/// assistant message is one turn; a turn with k tool calls counts 1/k toward
/// each call's cause, and a turn with none counts as other, so the causes sum
/// to the turns.
pub fn causes_from_transcript(text: &str) -> Causes {
    let mut out = no_causes();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(tools) = assistant_tools(&entry) else {
            continue;
        };
        if tools.is_empty() {
            *out.entry(Cause::Other).or_default() += 1.0;
            continue;
        }
        let share = 1.0 / tools.len() as f64;
        for t in tools {
            *out.entry(Cause::of_tool(t)).or_default() += share;
        }
    }
    out
}

/// One frontier request that reported usage.
#[derive(Debug, Clone, Serialize)]
pub struct Turn {
    pub model: String,
    /// What the request sent: see [`context_of`].
    pub context: u64,
    pub usage: Usage,
    /// At the model's verified list price; `None` when it has none.
    pub cost_usd: Option<f64>,
}

/// The context a request sent: uncached input, cache reads and cache writes.
pub fn context_of(u: &Usage) -> u64 {
    u.uncached_input + u.cache_read + u.cache_write
}

/// One run's profile.
#[derive(Debug, Clone, Serialize)]
pub struct RunProfile {
    pub run_id: String,
    pub task: String,
    pub lane: String,
    pub seed: u64,
    /// Frontier requests, as the proxy counted them.
    pub requests: u64,
    /// Requests that reported usage, in order (empty when the capture is gone).
    pub turns: Vec<Turn>,
    /// Usage per model over the run (the run record's, which its cost used).
    pub usage_by_model: BTreeMap<String, Usage>,
    /// Input (uncached, cache reads and writes) and output dollars at list
    /// price; `None` when a model has no verified price.
    pub input_usd: Option<f64>,
    pub output_usd: Option<f64>,
    /// Turns by cause (Duet lanes with a transcript).
    pub causes: Option<Causes>,
}

impl RunProfile {
    fn total(&self) -> Usage {
        let mut t = Usage::default();
        for u in self.usage_by_model.values() {
            t.add(*u);
        }
        t
    }
}

/// Input and output dollars of `by_model` at each model's own verified price.
fn priced(by_model: &BTreeMap<String, Usage>, prices: &PriceTable) -> Option<(f64, f64)> {
    by_model.iter().try_fold((0.0, 0.0), |(i, o), (model, u)| {
        let p = prices.verified(model).ok()?;
        Some((i + p.input_usd(*u), o + p.output_usd(*u)))
    })
}

/// A run's profile from its record, its proxy capture and (Duet lanes) its
/// transcript.
pub fn build(
    record: &RunRecord,
    requests: &[RequestUsage],
    transcript: Option<&str>,
    prices: &PriceTable,
) -> RunProfile {
    let turns = requests
        .iter()
        .filter_map(|r| {
            let usage = r.usage?;
            Some(Turn {
                cost_usd: prices.cost(&r.model, usage).ok(),
                model: r.model.clone(),
                context: context_of(&usage),
                usage,
            })
        })
        .collect();
    let dollars = priced(&record.usage_by_model, prices);
    RunProfile {
        run_id: record.run_id.clone(),
        task: record.task.clone(),
        lane: record.lane.clone(),
        seed: record.seed,
        requests: record.frontier_requests as u64,
        turns,
        usage_by_model: record.usage_by_model.clone(),
        input_usd: dollars.map(|d| d.0),
        output_usd: dollars.map(|d| d.1),
        causes: transcript.map(causes_from_transcript),
    }
}

/// The profile of the run in `run_dir` (its capture and transcript may be
/// missing: the profile then has no turns or no causes).
pub fn load(run_dir: &Path, record: &RunRecord, prices: &PriceTable) -> Result<RunProfile> {
    let proxy = run_dir.join("proxy");
    let requests = if proxy.join("requests.jsonl").is_file() {
        crate::cost::requests_from_proxy_log(&proxy)
            .with_context(|| format!("reading {}", proxy.display()))?
    } else {
        Vec::new()
    };
    let transcript = match record.lane_kind {
        crate::lanes::LaneKind::Duet => crate::ledger::newest_run_dir(&run_dir.join("workspace"))
            .and_then(|d| fs::read_to_string(d.join("transcript.jsonl")).ok()),
        crate::lanes::LaneKind::External => None,
    };
    Ok(build(record, &requests, transcript.as_deref(), prices))
}

/// Profiles of the valid `records` of the batch in `batch_dir`.
pub fn load_batch(
    batch_dir: &Path,
    records: &[RunRecord],
    prices: &PriceTable,
) -> Result<Vec<RunProfile>> {
    records
        .iter()
        .filter(|r| r.invalid.is_none())
        .map(|r| load(&batch_dir.join(&r.run_id), r, prices))
        .collect()
}

/// Profile of a lane over its runs (of one task, or all).
#[derive(Debug, Clone, Serialize)]
pub struct ProfileSummary {
    pub lane: String,
    /// `None`: every task.
    pub task: Option<String>,
    pub runs: usize,
    /// Mean frontier requests per run.
    pub turns: f64,
    /// Context per turn over every turn of these runs (pooled).
    pub context_mean: Option<f64>,
    pub context_p90: Option<f64>,
    /// Mean per run of the context summed over its turns.
    pub request_tokens: f64,
    /// Share of all context read from the provider's cache.
    pub cached_share: Option<f64>,
    /// Mean output tokens per run.
    pub output_tokens: f64,
    /// Input dollars over all frontier dollars; `None` when a run is unpriced.
    pub input_cost_share: Option<f64>,
    /// Mean turns per run by cause, over the runs with a transcript.
    pub causes: Option<Causes>,
    pub runs_with_causes: usize,
}

/// Summarizes `profiles` (one lane, and one task or all).
pub fn summarize(lane: &str, task: Option<&str>, profiles: &[&RunProfile]) -> ProfileSummary {
    let n = profiles.len().max(1) as f64;
    let contexts: Vec<f64> = profiles
        .iter()
        .flat_map(|p| p.turns.iter().map(|t| t.context as f64))
        .collect();
    let mut total = Usage::default();
    for p in profiles {
        total.add(p.total());
    }
    let context = context_of(&total);
    let dollars: Option<Vec<(f64, f64)>> = profiles
        .iter()
        .map(|p| Some((p.input_usd?, p.output_usd?)))
        .collect();
    let input_cost_share = dollars.and_then(|d| {
        let (i, o) = d.iter().fold((0.0, 0.0), |(a, b), (i, o)| (a + i, b + o));
        (i + o > 0.0).then(|| i / (i + o))
    });
    let with_causes: Vec<&Causes> = profiles.iter().filter_map(|p| p.causes.as_ref()).collect();
    let causes = (!with_causes.is_empty()).then(|| {
        let mut m = no_causes();
        for c in &with_causes {
            for (k, v) in *c {
                *m.entry(*k).or_default() += v;
            }
        }
        for v in m.values_mut() {
            *v /= with_causes.len() as f64;
        }
        m
    });
    ProfileSummary {
        lane: lane.to_owned(),
        task: task.map(str::to_owned),
        runs: profiles.len(),
        turns: profiles.iter().map(|p| p.requests as f64).sum::<f64>() / n,
        context_mean: (!contexts.is_empty())
            .then(|| contexts.iter().sum::<f64>() / contexts.len() as f64),
        context_p90: stats::percentile(&contexts, 0.9),
        request_tokens: context as f64 / n,
        cached_share: (context > 0).then(|| total.cache_read as f64 / context as f64),
        output_tokens: total.output as f64 / n,
        input_cost_share,
        causes,
        runs_with_causes: with_causes.len(),
    }
}

/// Every lane over all its tasks, then each (lane, task), sorted by name.
pub fn summarize_all(profiles: &[RunProfile]) -> Vec<ProfileSummary> {
    let mut by_lane: BTreeMap<&str, BTreeMap<&str, Vec<&RunProfile>>> = BTreeMap::new();
    for p in profiles {
        by_lane
            .entry(&p.lane)
            .or_default()
            .entry(&p.task)
            .or_default()
            .push(p);
    }
    let mut out = Vec::new();
    for (lane, tasks) in by_lane {
        let all: Vec<&RunProfile> = tasks.values().flatten().copied().collect();
        out.push(summarize(lane, None, &all));
        for (task, ps) in tasks {
            out.push(summarize(lane, Some(task), &ps));
        }
    }
    out
}

/// Input and output dollars of `by_model` if every model had cost `price`.
pub fn project_usage(price: &Price, by_model: &BTreeMap<String, Usage>) -> (f64, f64) {
    by_model.values().fold((0.0, 0.0), |(i, o), u| {
        (i + price.input_usd(*u), o + price.output_usd(*u))
    })
}

/// One lane's frontier dollars under one price table.
#[derive(Debug, Clone, Serialize)]
pub struct LaneCost {
    pub lane: String,
    /// The lane's valid runs.
    pub runs: usize,
    /// Mean frontier dollars per run; `None` when a run is unpriced.
    pub mean_usd: Option<f64>,
    /// Input dollars over all dollars.
    pub input_share: Option<f64>,
    /// Paired Σlane / Σreference over the same (task, seed), with its 95%
    /// bootstrap interval; `None` for the reference itself or without pairs.
    pub ratio_vs_reference: Option<Interval>,
}

/// Every lane's cost when each run costs `cost(run)` (input, output dollars;
/// `None`: unpriced).
fn lane_costs(
    profiles: &[RunProfile],
    reference: &str,
    cost: &dyn Fn(&RunProfile) -> Option<(f64, f64)>,
) -> Vec<LaneCost> {
    let mut by_lane: BTreeMap<&str, Vec<&RunProfile>> = BTreeMap::new();
    for p in profiles {
        by_lane.entry(&p.lane).or_default().push(p);
    }
    let refs: BTreeMap<(&str, u64), f64> = by_lane
        .get(reference)
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let (i, o) = cost(p)?;
            Some(((p.task.as_str(), p.seed), i + o))
        })
        .collect();
    by_lane
        .iter()
        .map(|(lane, ps)| {
            let costs: Option<Vec<(f64, f64)>> = ps.iter().map(|p| cost(p)).collect();
            let (mean_usd, input_share) = match &costs {
                Some(c) if !c.is_empty() => {
                    let (i, o) = c.iter().fold((0.0, 0.0), |(a, b), (i, o)| (a + i, b + o));
                    (
                        Some((i + o) / c.len() as f64),
                        (i + o > 0.0).then(|| i / (i + o)),
                    )
                }
                _ => (None, None),
            };
            let ratio_vs_reference = (*lane != reference)
                .then(|| {
                    let pairs: Vec<(f64, f64)> = ps
                        .iter()
                        .filter_map(|p| {
                            let r = refs.get(&(p.task.as_str(), p.seed))?;
                            let (i, o) = cost(p)?;
                            Some((i + o, *r))
                        })
                        .collect();
                    stats::paired_ratio(&pairs, BOOTSTRAP_ITERS, ALPHA, 21)
                })
                .flatten();
            LaneCost {
                lane: (*lane).to_owned(),
                runs: ps.len(),
                mean_usd,
                input_share,
                ratio_vs_reference,
            }
        })
        .collect()
}

/// A price table the batch is projected at.
#[derive(Debug, Clone, Serialize)]
pub struct Projection {
    /// The `pricing.toml` row whose list prices every run is charged at.
    pub model: String,
    /// USD per million tokens.
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
    pub source: String,
    pub observed: String,
    pub lanes: Vec<LaneCost>,
}

/// The batch at its recorded prices and at each of `tables`.
#[derive(Debug, Clone, Serialize)]
pub struct Projections {
    pub reference: String,
    /// Each run at its own models' list prices.
    pub recorded: Vec<LaneCost>,
    pub tables: Vec<Projection>,
}

pub fn project(profiles: &[RunProfile], tables: &[Price], reference: &str) -> Projections {
    Projections {
        reference: reference.to_owned(),
        recorded: lane_costs(profiles, reference, &|p| {
            Some((p.input_usd?, p.output_usd?))
        }),
        tables: tables
            .iter()
            .map(|price| Projection {
                model: price.model.clone(),
                input: price.input,
                cache_read: price.cache_read,
                cache_write: price.cache_write,
                output: price.output,
                source: price.source.clone(),
                observed: price.observed.clone(),
                lanes: lane_costs(profiles, reference, &|p| {
                    Some(project_usage(price, &p.usage_by_model))
                }),
            })
            .collect(),
    }
}

/// The prices `--project-prices` names: `flagships` ([`FLAGSHIPS`]), the path
/// of a price table (all its verified rows), or model names in `prices`.
/// Every price must be verified.
pub fn resolve_tables(spec: &[String], prices: &PriceTable) -> Result<Vec<Price>> {
    let mut out: Vec<Price> = Vec::new();
    for entry in spec.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let found: Vec<Price> = if entry == "flagships" {
            FLAGSHIPS
                .iter()
                .map(|m| prices.verified(m).cloned())
                .collect::<Result<_>>()?
        } else if Path::new(entry).is_file() {
            let table = PriceTable::load(Path::new(entry))?;
            let verified: Vec<Price> = table.prices.into_iter().filter(|p| p.verified).collect();
            if verified.is_empty() {
                bail!("{entry} has no verified price");
            }
            verified
        } else {
            vec![prices.verified(entry)?.clone()]
        };
        for p in found {
            if !out.iter().any(|o| o.model == p.model) {
                out.push(p);
            }
        }
    }
    Ok(out)
}

/// Tokens in thousands, or millions from a million on.
fn kilo(x: f64) -> String {
    if x >= 1e6 {
        format!("{:.2}M", x / 1e6)
    } else {
        format!("{:.1}K", x / 1000.0)
    }
}

fn percent(x: Option<f64>) -> String {
    x.map_or("—".into(), |v| format!("{:.0}%", 100.0 * v))
}

/// The cost profile section of a batch report.
pub fn render_profile(summaries: &[ProfileSummary]) -> String {
    let mut s = String::from(
        "\n## Cost profile\n\nPer run means. Turns are frontier requests; the context of a turn is \
         what it sent (uncached input + cache reads + cache writes, from the provider's usage in the \
         proxy capture), its mean and 90th percentile over every turn of the runs. Request tokens \
         are the context summed over a run's turns. Turns by cause come from Duet's transcript \
         (sub-agents included): a turn with k tool calls counts 1/k toward each call's cause \
         (reading: read_file, read_raw, list_files, search, diff, git history, web, code_nav; \
         editing: edit_file, write_file, edit_protected, rename, git_commit; commands: run_command; \
         other: finish, delegate, MCP tools, no tool call).\n\n",
    );
    s.push_str(
        "| Lane | Task | Runs | Turns | Context/turn | p90 | Request tokens | Cached | Output | Input share of $ |",
    );
    for c in Cause::ALL {
        let _ = write!(s, " {} |", c.label());
    }
    s.push_str("\n|---|---|---|---|---|---|---|---|---|---|");
    s.push_str(&"---|".repeat(Cause::ALL.len()));
    s.push('\n');
    for p in summaries {
        let _ = write!(
            s,
            "| {} | {} | {} | {:.1} | {} | {} | {} | {} | {} | {} |",
            p.lane,
            p.task.as_deref().unwrap_or("**all**"),
            p.runs,
            p.turns,
            p.context_mean.map_or("—".into(), kilo),
            p.context_p90.map_or("—".into(), kilo),
            kilo(p.request_tokens),
            percent(p.cached_share),
            kilo(p.output_tokens),
            percent(p.input_cost_share),
        );
        for c in Cause::ALL {
            let _ = write!(
                s,
                " {} |",
                p.causes.as_ref().map_or("—".into(), |m| format!(
                    "{:.1}",
                    m.get(&c).copied().unwrap_or(0.0)
                ))
            );
        }
        s.push('\n');
    }
    s
}

fn usd(x: Option<f64>) -> String {
    x.map_or("unknown".into(), |v| format!("${v:.4}"))
}

fn ratio(x: Option<&Interval>) -> String {
    x.map_or("—".into(), |i| {
        format!("{:.2} [{:.2}, {:.2}]", i.estimate, i.lower, i.upper)
    })
}

/// The price projection section of a batch report.
pub fn render_projection(p: &Projections) -> String {
    let mut s = String::from(
        "\n## Price projection\n\nEvery run's recorded frontier tokens (uncached input, cache reads, \
         cache writes, output; reasoning tokens are part of every provider's reported output and are \
         charged as output) priced at other list prices. Tokenizer differences are ignored: another \
         model's tokenizer would count the same text differently. The recorded cache split is kept: \
         a provider whose cache writes cost more than input (Anthropic's 5-minute write is 1.25×) \
         would bill part of the uncached input as writes, so such a projection is low by up to a \
         quarter of its uncached-input dollars. Frontier dollars only (local electricity is left out).\n\n",
    );
    let header = |s: &mut String, count: &str| {
        let _ = write!(s, "| Lane | {count} | recorded |");
        for t in &p.tables {
            let _ = write!(s, " {} |", t.model);
        }
        s.push_str("\n|---|---|---|");
        s.push_str(&"---|".repeat(p.tables.len()));
        s.push('\n');
    };
    s.push_str("Mean frontier dollars per run:\n\n");
    header(&mut s, "Runs");
    for (i, rec) in p.recorded.iter().enumerate() {
        let _ = write!(s, "| {} | {} | {} |", rec.lane, rec.runs, usd(rec.mean_usd));
        for t in &p.tables {
            let _ = write!(s, " {} |", usd(t.lanes[i].mean_usd));
        }
        s.push('\n');
    }
    let _ = write!(
        s,
        "\nPaired ratio against `{}` (Σ lane / Σ reference over runs of the same task and seed; \
         95% bootstrap interval):\n\n",
        p.reference
    );
    header(&mut s, "Pairs");
    for (i, rec) in p.recorded.iter().enumerate() {
        if rec.lane == p.reference {
            continue;
        }
        let pairs = rec
            .ratio_vs_reference
            .as_ref()
            .or_else(|| {
                p.tables
                    .first()
                    .and_then(|t| t.lanes[i].ratio_vs_reference.as_ref())
            })
            .map_or(0, |r| r.n);
        let _ = write!(
            s,
            "| {} | {pairs} | {} |",
            rec.lane,
            ratio(rec.ratio_vs_reference.as_ref())
        );
        for t in &p.tables {
            let _ = write!(s, " {} |", ratio(t.lanes[i].ratio_vs_reference.as_ref()));
        }
        s.push('\n');
    }
    s.push_str("\nPrices (USD per million tokens: input / cache read / cache write / output):\n\n");
    for t in &p.tables {
        let _ = writeln!(
            s,
            "- `{}`: {:.2} / {:.2} / {:.2} / {:.2} ({}, observed {})",
            t.model, t.input, t.cache_read, t.cache_write, t.output, t.source, t.observed
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn u(uncached: u64, read: u64, write: u64, output: u64) -> Usage {
        Usage {
            uncached_input: uncached,
            cache_read: read,
            cache_write: write,
            output,
        }
    }

    fn prices() -> PriceTable {
        toml::from_str(
            r#"
electricity_usd_per_kwh = 0.2
[[price]]
model = "cheap"
input = 1.0
cache_read = 0.1
cache_write = 1.0
output = 4.0
source = "test"
observed = "2026-09-26"
verified = true
[[price]]
model = "dear"
input = 5.0
cache_read = 0.5
cache_write = 6.25
output = 25.0
source = "test"
observed = "2026-09-26"
verified = true
[[price]]
model = "guess"
input = 1.0
cache_read = 0.1
cache_write = 1.0
output = 1.0
source = "placeholder"
observed = "2026-09-26"
verified = false
"#,
        )
        .unwrap()
    }

    fn record(lane: &str, task: &str, seed: u64, usage: Usage) -> RunRecord {
        serde_json::from_value(json!({
            "task": task, "lane": lane, "lane_kind": "duet", "seed": seed,
            "run_id": format!("{task}-{lane}-s{seed}"), "exit_code": 0, "timed_out": false,
            "wall_seconds": 1.0, "grade": null, "leaks": [], "frontier_requests": 2,
            "usage_by_model": {"cheap": usage}, "unreported_requests": 0,
            "frontier_cost_usd": null, "electricity_usd": 0.0, "total_cost_usd": null,
            "error": null
        }))
        .unwrap()
    }

    fn requests(turns: &[Usage]) -> Vec<RequestUsage> {
        turns
            .iter()
            .map(|t| RequestUsage {
                model: "cheap".into(),
                usage: Some(*t),
            })
            .collect()
    }

    #[test]
    fn token_counts_read_as_thousands_or_millions() {
        assert_eq!(kilo(12_345.0), "12.3K");
        assert_eq!(kilo(11_960_400.0), "11.96M");
    }

    #[test]
    fn tools_map_to_causes() {
        for (tool, cause) in [
            ("read_file", Cause::Reading),
            ("git_log", Cause::Reading),
            ("web_fetch", Cause::Reading),
            ("edit_file", Cause::Editing),
            ("edit_protected", Cause::Editing),
            ("run_command", Cause::Commands),
            ("ask_local", Cause::AskLocal),
            ("finish", Cause::Other),
            ("delegate", Cause::Other),
            ("mcp__fs__read_file", Cause::Other),
        ] {
            assert_eq!(Cause::of_tool(tool), cause, "{tool}");
        }
    }

    #[test]
    fn a_turn_is_split_between_the_causes_of_its_calls() {
        let assistant = |tools: &[&str]| {
            let calls: Vec<Value> = tools
                .iter()
                .map(|t| json!({"id": "c", "name": t, "arguments": {}}))
                .collect();
            json!({"kind": "item", "item": {"type": "assistant", "text": "", "tool_calls": calls}})
        };
        let lines = [
            json!({"kind": "start", "objective": "x", "mode": "hybrid", "frontier_model": "m"}),
            json!({"kind": "usage", "turn": 1, "usage": {}, "cost_usd": 0.0, "interventions": []}),
            assistant(&["read_file", "read_file", "ask_local", "run_command"]),
            json!({"kind": "item", "item": {"type": "tool_result", "call_id": "c", "content": "x"}}),
            assistant(&["edit_file"]),
            // A sub-agent's turn is nested under its id.
            json!({"kind": "subagent", "child": "a1", "entry": assistant(&["search"])}),
            assistant(&[]),
            "not json".into(),
        ];
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let c = causes_from_transcript(&text);
        assert!((c[&Cause::Reading] - 1.5).abs() < 1e-12, "{c:?}");
        assert!((c[&Cause::AskLocal] - 0.25).abs() < 1e-12);
        assert!((c[&Cause::Commands] - 0.25).abs() < 1e-12);
        assert!((c[&Cause::Editing] - 1.0).abs() < 1e-12);
        assert!((c[&Cause::Other] - 1.0).abs() < 1e-12);
        // Four turns in all.
        assert!((c.values().sum::<f64>() - 4.0).abs() < 1e-12);
    }

    #[test]
    fn per_turn_context_is_pooled_across_runs() {
        let p = prices();
        // Run 1: turns of 1000 and 3000 context; run 2: one of 6000 and one unreported.
        let a = build(
            &record("hy", "S1", 1, u(1000, 3000, 0, 300)),
            &requests(&[u(1000, 0, 0, 100), u(0, 3000, 0, 200)]),
            None,
            &p,
        );
        let mut r2 = record("hy", "S1", 2, u(2000, 4000, 0, 100));
        r2.frontier_requests = 2;
        let mut req2 = requests(&[u(2000, 4000, 0, 100)]);
        req2.push(RequestUsage {
            model: "cheap".into(),
            usage: None,
        });
        let b = build(&r2, &req2, Some(""), &p);
        assert_eq!(context_of(&a.total()), 4000);
        // Per turn cost at the model's price: 1000 × $1/M + 100 × $4/M.
        assert!((a.turns[0].cost_usd.unwrap() - 0.0014).abs() < 1e-12);
        let s = summarize("hy", None, &[&a, &b]);
        assert_eq!(s.runs, 2);
        assert!((s.turns - 2.0).abs() < 1e-12);
        // Contexts 1000, 3000, 6000: mean 3333.3, p90 between 3000 and 6000.
        assert!((s.context_mean.unwrap() - 10_000.0 / 3.0).abs() < 1e-9);
        assert!((s.context_p90.unwrap() - 5400.0).abs() < 1e-9);
        assert!((s.request_tokens - 5000.0).abs() < 1e-12);
        assert!((s.cached_share.unwrap() - 0.7).abs() < 1e-12);
        assert!((s.output_tokens - 200.0).abs() < 1e-12);
        // Input $: 3000×1 + 7000×0.1 = 3700e-6; output $: 400×4 = 1600e-6.
        assert!((s.input_cost_share.unwrap() - 3700.0 / 5300.0).abs() < 1e-12);
        // Only run 2 has a transcript (an empty one: no turns by cause).
        assert_eq!(s.runs_with_causes, 1);
        assert_eq!(s.causes.unwrap()[&Cause::Reading], 0.0);
    }

    #[test]
    fn an_unpriced_model_leaves_dollars_unknown_but_tokens_counted() {
        let p = prices();
        let mut r = record("hy", "S1", 1, u(1000, 0, 0, 10));
        r.usage_by_model = [("guess".to_owned(), u(1000, 0, 0, 10))].into();
        let prof = build(&r, &[], None, &p);
        assert!(prof.input_usd.is_none() && prof.output_usd.is_none());
        let s = summarize("hy", Some("S1"), &[&prof]);
        assert_eq!(s.input_cost_share, None);
        assert!((s.request_tokens - 1000.0).abs() < 1e-12);
        assert_eq!(s.context_mean, None, "no capture, no per-turn context");
    }

    #[test]
    fn summaries_cover_each_lane_then_each_task() {
        let p = prices();
        let ps: Vec<RunProfile> = [("b", "S1"), ("a", "S2"), ("a", "S1"), ("a", "S1")]
            .iter()
            .enumerate()
            .map(|(i, (lane, task))| {
                build(&record(lane, task, i as u64, u(10, 0, 0, 1)), &[], None, &p)
            })
            .collect();
        let keys: Vec<(String, Option<String>, usize)> = summarize_all(&ps)
            .into_iter()
            .map(|s| (s.lane, s.task, s.runs))
            .collect();
        assert_eq!(
            keys,
            [
                ("a".into(), None, 3),
                ("a".into(), Some("S1".into()), 2),
                ("a".into(), Some("S2".into()), 1),
                ("b".into(), None, 1),
                ("b".into(), Some("S1".into()), 1),
            ]
        );
    }

    #[test]
    fn projection_charges_recorded_tokens_at_each_price() {
        let p = prices();
        let dear = p.verified("dear").unwrap();
        let by_model: BTreeMap<String, Usage> = [
            ("cheap".to_owned(), u(1_000_000, 2_000_000, 0, 100_000)),
            ("other".to_owned(), u(0, 0, 400_000, 0)),
        ]
        .into();
        let (i, o) = project_usage(dear, &by_model);
        // 1M × 5 + 2M × 0.5 + 0.4M × 6.25 = 8.5; 0.1M × 25 = 2.5.
        assert!((i - 8.5).abs() < 1e-9 && (o - 2.5).abs() < 1e-9, "{i} {o}");
    }

    #[test]
    fn projected_ratios_are_paired_by_task_and_seed() {
        let p = prices();
        let mut ps = Vec::new();
        // The candidate sends twice the reference's input and the same output,
        // on two seeds; a third candidate seed has no reference run.
        for seed in 1..=2 {
            ps.push(build(
                &record("cand", "S1", seed, u(2_000_000, 0, 0, 100_000)),
                &[],
                None,
                &p,
            ));
            ps.push(build(
                &record("ref", "S1", seed, u(1_000_000, 0, 0, 100_000)),
                &[],
                None,
                &p,
            ));
        }
        ps.push(build(
            &record("cand", "S1", 3, u(9_000_000, 0, 0, 0)),
            &[],
            None,
            &p,
        ));
        let tables = resolve_tables(&["dear".into()], &p).unwrap();
        let proj = project(&ps, &tables, "ref");
        let cand = &proj.recorded[0];
        assert_eq!((cand.lane.as_str(), cand.runs), ("cand", 3));
        // Recorded (cheap): pairs (2.4, 1.4) twice.
        let r = cand.ratio_vs_reference.unwrap();
        assert_eq!(r.n, 2);
        assert!((r.estimate - 2.4 / 1.4).abs() < 1e-9);
        // At the dear price: (10 + 2.5) / (5 + 2.5).
        let dear = &proj.tables[0].lanes[0];
        assert!((dear.ratio_vs_reference.unwrap().estimate - 12.5 / 7.5).abs() < 1e-9);
        // Mean per run over all three candidate runs: (12.5 + 12.5 + 45) / 3.
        assert!((dear.mean_usd.unwrap() - 70.0 / 3.0).abs() < 1e-9);
        assert!((dear.input_share.unwrap() - 65.0 / 70.0).abs() < 1e-9);
        assert!(proj.tables[0].lanes[1].ratio_vs_reference.is_none());
        let md = render_projection(&proj);
        assert!(md.contains("| cand | 2 | 1.71 ["), "{md}");
        assert!(md.contains("| ref | 2 | $1.4000 | $7.5000 |"), "{md}");
        assert!(md.contains("Tokenizer differences are ignored"), "{md}");
        assert!(md.contains("- `dear`: 5.00 / 0.50 / 6.25 / 25.00 (test, observed 2026-09-26)"));
    }

    #[test]
    fn tables_resolve_from_names_a_file_or_the_flagships() {
        let p = prices();
        let names: Vec<String> =
            resolve_tables(&["dear".into(), "cheap".into(), "dear".into()], &p)
                .unwrap()
                .into_iter()
                .map(|t| t.model)
                .collect();
        assert_eq!(names, ["dear", "cheap"]);
        assert!(resolve_tables(&["guess".into()], &p).is_err(), "unverified");
        assert!(resolve_tables(&["nothing".into()], &p).is_err());
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("other.toml");
        fs::write(
            &file,
            "electricity_usd_per_kwh = 0.1\n[[price]]\nmodel = \"x\"\ninput = 1.0\ncache_read = 0.1\n\
             cache_write = 1.0\noutput = 2.0\nsource = \"s\"\nobserved = \"d\"\nverified = true\n",
        )
        .unwrap();
        let from_file = resolve_tables(&[file.to_string_lossy().into_owned()], &p).unwrap();
        assert_eq!(from_file[0].model, "x");
        // The flagships are rows of the harness's own price table.
        let real = PriceTable::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/pricing.toml"
        )))
        .unwrap();
        let flagships = resolve_tables(&["flagships".into()], &real).unwrap();
        assert_eq!(
            flagships
                .iter()
                .map(|t| t.model.as_str())
                .collect::<Vec<_>>(),
            FLAGSHIPS
        );
    }

    #[test]
    fn projection_prices_match_duets_own_cited_prices() {
        // The harness does not link Duet at run time; this check keeps its
        // table and the agent's built-in prices (which `duet doctor` cites) equal.
        let real = PriceTable::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/pricing.toml"
        )))
        .unwrap();
        for row in real.prices.iter().filter(|p| p.verified) {
            let Some(b) = duet_provider::price::builtin(&row.model) else {
                continue;
            };
            assert_eq!(
                (row.input, row.cache_read, row.cache_write, row.output),
                (b.input, b.cache_read, b.cache_write, b.output),
                "{}",
                row.model
            );
        }
        for m in FLAGSHIPS {
            assert!(duet_provider::price::builtin(m).is_some(), "{m}");
        }
    }

    #[test]
    fn a_run_directory_loads_from_its_capture_and_transcript() {
        let d = tempfile::tempdir().unwrap();
        let run = d.path().join("S1-hy-s1");
        let proxy = run.join("proxy");
        fs::create_dir_all(proxy.join("requests")).unwrap();
        fs::create_dir_all(proxy.join("responses")).unwrap();
        let mut log = String::new();
        let bodies = [
            // A stream with usage; a stream cut short (no usage).
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5000,\"completion_tokens\":40,\
             \"prompt_tokens_details\":{\"cached_tokens\":4000}}}\n\ndata: [DONE]\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"par\"}}]}\n\n",
        ];
        for (i, body) in bodies.iter().enumerate() {
            let seq = i + 1;
            log.push_str(
                &json!({"seq": seq, "unix_ms": 1, "method": "POST", "path": "/chat/completions",
                        "bytes": 10, "sha256": "x", "status": 200, "leaked": []})
                .to_string(),
            );
            log.push('\n');
            fs::write(
                proxy.join(format!("requests/{seq:05}.body")),
                json!({"model": "cheap", "messages": [{"role": "user", "content": "hi"}]})
                    .to_string(),
            )
            .unwrap();
            fs::write(proxy.join(format!("responses/{seq:05}.body")), body).unwrap();
        }
        fs::write(proxy.join("requests.jsonl"), log).unwrap();
        let duet_run = run.join("workspace/.duet/runs/20260926-1");
        fs::create_dir_all(&duet_run).unwrap();
        fs::write(duet_run.join("summary.json"), "{}").unwrap();
        fs::write(
            duet_run.join("transcript.jsonl"),
            json!({"kind": "item", "item": {"type": "assistant", "text": "",
                   "tool_calls": [{"id": "c", "name": "run_command", "arguments": {}}]}})
            .to_string(),
        )
        .unwrap();
        let rec = record("hy", "S1", 1, u(1000, 4000, 0, 40));
        let prof = load(&run, &rec, &prices()).unwrap();
        assert_eq!(prof.turns.len(), 1, "the cut stream reported nothing");
        assert_eq!(prof.turns[0].usage, u(1000, 4000, 0, 40));
        assert_eq!(prof.turns[0].model, "cheap");
        assert_eq!(prof.causes.as_ref().unwrap()[&Cause::Commands], 1.0);
        // Without a capture or a transcript the profile keeps the recorded totals.
        let bare = load(&d.path().join("gone"), &rec, &prices()).unwrap();
        assert!(bare.turns.is_empty() && bare.causes.is_none());
        assert_eq!(context_of(&bare.total()), 5000);
    }

    #[test]
    fn the_profile_table_shows_all_then_each_task() {
        let p = prices();
        let a = build(
            &record("hy", "S1", 1, u(1000, 3000, 0, 300)),
            &requests(&[u(1000, 0, 0, 100), u(0, 3000, 0, 200)]),
            Some(
                &[
                    json!({"kind": "item", "item": {"type": "assistant", "text": "",
                          "tool_calls": [{"id": "c", "name": "ask_local", "arguments": {}}]}})
                    .to_string(),
                ]
                .join("\n"),
            ),
            &p,
        );
        let md = render_profile(&summarize_all(&[a]));
        assert!(md.contains("## Cost profile"), "{md}");
        assert!(
            md.contains("| hy | **all** | 1 | 2.0 | 2.0K | 2.8K | 4.0K | 75% | 0.3K | 52% | 0.0 | 0.0 | 0.0 | 1.0 | 0.0 |"),
            "{md}"
        );
        assert!(md.contains("| hy | S1 | 1 |"), "{md}");
    }
}
