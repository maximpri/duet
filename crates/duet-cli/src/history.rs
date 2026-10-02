// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only history. Never recover a transcript while another run can append.

use anyhow::{Context, Result, ensure};
use duet_fs::pinned::PinnedParent;
use rustix::fs::{Dir, Mode, OFlags};
use serde::Serialize;
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_MANIFEST: u64 = 1024 * 1024;
const MAX_TRANSCRIPT: u64 = 16 * 1024 * 1024;
const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RUNS: usize = 1_000;
const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Open a saved conversation by its ID.
    pub id: Option<String>,
    /// Number of recent sessions and runs to show.
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=1000))]
    pub limit: u16,
    /// Find words in saved titles and conversation messages.
    #[arg(long)]
    pub search: Option<String>,
    /// Print structured history.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Serialize)]
struct Message {
    speaker: &'static str,
    text: String,
}

#[derive(Debug, Serialize)]
struct GoalView {
    objective: String,
    status: &'static str,
    turns: u64,
    max_turns: u64,
    summary: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct Record {
    id: String,
    title: String,
    kind: &'static str,
    status: &'static str,
    started_at: String,
    turns: u64,
    requests: u64,
    cost_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    goal: Option<GoalView>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    previous_goals: Vec<GoalView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resume_command: Option<String>,
    incomplete: bool,
    notes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    messages: Vec<Message>,
}

#[derive(Debug, Default, Serialize)]
struct Listing {
    runs: Vec<Record>,
    notes: Vec<String>,
}

pub(crate) fn show(ws: &Path, args: Args) -> Result<()> {
    println!("{}", render(ws, args)?);
    Ok(())
}

pub(crate) fn render(ws: &Path, args: Args) -> Result<String> {
    if let Some(id) = args.id {
        valid_id(&id)?;
        let mut budget = MAX_TRANSCRIPT;
        let (record, _) = read_record(ws, &id, true, None, &mut budget)?;
        if args.json {
            Ok(serde_json::to_string_pretty(&record)?)
        } else {
            Ok(detail(&record))
        }
    } else {
        let listing = list(ws, args.limit as usize, args.search.as_deref())?;
        if args.json {
            Ok(serde_json::to_string_pretty(&listing)?)
        } else {
            Ok(render_listing(&listing, args.search.is_some()))
        }
    }
}

pub(crate) fn summary(ws: &Path) -> Result<String> {
    render(
        ws,
        Args {
            id: None,
            limit: 20,
            search: None,
            json: false,
        },
    )
}

fn valid_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "Invalid history ID. Copy an ID shown by duet history."
    );
    Ok(())
}

/// Directory handles prevent a symlink swap from redirecting the enumeration.
fn run_ids(ws: &Path) -> Result<(Vec<String>, bool)> {
    let mut dir = rustix::fs::open(ws, DIR_FLAGS, Mode::empty())
        .context("Cannot open this workspace's history")?;
    for name in [".duet", "runs"] {
        dir = match rustix::fs::openat(&dir, name, DIR_FLAGS, Mode::empty()) {
            Ok(dir) => dir,
            Err(rustix::io::Errno::NOENT) => return Ok((Vec::new(), false)),
            Err(error) => return Err(error).context("Cannot open saved history safely"),
        };
    }
    let mut entries = Dir::new(dir)?;
    let mut ids = std::collections::BTreeSet::new();
    let mut capped = false;
    while let Some(entry) = entries.read() {
        let entry = entry?;
        let Ok(id) = entry.file_name().to_str() else {
            continue;
        };
        if valid_id(id).is_err() || entry.file_type() == rustix::fs::FileType::Symlink {
            continue;
        }
        ids.insert(id.to_owned());
        if ids.len() > MAX_RUNS {
            ids.pop_first();
            capped = true;
        }
    }
    Ok((ids.into_iter().rev().collect(), capped))
}

fn list(ws: &Path, limit: usize, search: Option<&str>) -> Result<Listing> {
    let (ids, capped) = run_ids(ws)?;
    let query = search.map(str::to_lowercase);
    let mut out = Listing::default();
    let mut budget = MAX_SCAN_BYTES;
    let mut skipped = 0;
    let mut partial = 0;
    for id in ids {
        if out.runs.len() >= limit {
            break;
        }
        if budget == 0 {
            out.notes.push(
                "Reached the history read limit; narrow your search or open an ID directly.".into(),
            );
            break;
        }
        match read_record(ws, &id, false, query.as_deref(), &mut budget) {
            Ok((record, matched)) => {
                partial += usize::from(record.incomplete);
                if matched {
                    out.runs.push(record);
                }
            }
            Err(_) => skipped += 1,
        }
    }
    if skipped > 0 {
        out.notes.push(format!(
            "Skipped {skipped} unreadable or invalid history record(s)."
        ));
    }
    if partial > 0 {
        out.notes.push(format!("Read {partial} transcript(s) partially; search and totals cover their complete readable entries only."));
    }
    if capped {
        out.notes.push(format!(
            "Searched the newest {MAX_RUNS} saved IDs; older entries remain on disk."
        ));
    }
    Ok(out)
}

fn artifact(id: &str, name: &str) -> PathBuf {
    Path::new(".duet/runs").join(id).join(name)
}

/// Every component refuses symlinks. The file handle is read-only, capped,
/// and its incomplete final line is never repaired or truncated on disk.
fn transcript(ws: &Path, id: &str, cap: u64) -> Result<(Vec<u8>, bool)> {
    let pinned = PinnedParent::open(ws, &artifact(id, "transcript.jsonl"), false)?;
    if pinned.file_type()?.is_none() {
        return Ok((Vec::new(), false));
    }
    let file = pinned.open_read()?;
    let mut bytes = Vec::new();
    file.take(cap + 1).read_to_end(&mut bytes)?;
    let limited = bytes.len() as u64 > cap;
    bytes.truncate(cap as usize);
    Ok((bytes, limited))
}

fn goals(ws: &Path, id: &str, budget: &mut u64) -> Result<(Option<GoalView>, Vec<GoalView>)> {
    let path = artifact(id, "goals.json");
    let pinned = PinnedParent::open(ws, &path, false)?;
    if pinned.file_type()?.is_none() {
        return Ok((None, Vec::new()));
    }
    let bytes = duet_fs::read_file(ws, &path, (4 * 1024 * 1024).min(*budget))?;
    *budget = budget.saturating_sub(bytes.len() as u64);
    let saved: Value = serde_json::from_slice(&bytes)?;
    ensure!(saved["version"] == 1, "Unknown goal history version");
    let current = if saved["current"].is_null() {
        None
    } else {
        Some(goal_view(serde_json::from_value(saved["current"].clone())?))
    };
    let previous = saved.get("history").and_then(Value::as_array);
    ensure!(
        previous.is_none_or(|goals| goals.len() <= MAX_RUNS),
        "Goal history exceeds its limit"
    );
    let previous = previous
        .into_iter()
        .flatten()
        .map(|value| serde_json::from_value(value.clone()).map(goal_view))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((current, previous))
}

fn goal_view(goal: crate::goals::Goal) -> GoalView {
    use crate::goals::State;
    let status = match goal.state {
        State::Active => "Working or interrupted",
        State::Waiting => "Waiting for your answer",
        State::Paused => "Paused",
        State::Completed => "Completed",
        State::Exhausted => "Turn limit reached",
        State::Cancelled => "Cancelled",
    };
    GoalView {
        objective: duet_tui::term::safe(&goal.objective),
        status,
        turns: goal.turns,
        max_turns: goal.max_turns,
        summary: goal.summary.map(|s| duet_tui::term::safe(&s)),
        reason: goal.reason.map(|s| duet_tui::term::safe(&s)),
    }
}

fn read_record(
    ws: &Path,
    id: &str,
    detailed: bool,
    query: Option<&str>,
    budget: &mut u64,
) -> Result<(Record, bool)> {
    valid_id(id)?;
    let bytes = duet_fs::read_file(ws, &artifact(id, "run.json"), MAX_MANIFEST)
        .with_context(|| format!("Cannot read saved conversation {id}"))?;
    let manifest: Value =
        serde_json::from_slice(&bytes).context("Invalid saved conversation metadata")?;
    *budget = budget.saturating_sub(bytes.len() as u64);
    ensure!(manifest.is_object(), "Invalid saved conversation metadata");
    ensure!(
        manifest
            .get("run_id")
            .and_then(Value::as_str)
            .is_none_or(|stored| stored == id),
        "Saved conversation metadata names another ID"
    );
    let mut session = manifest
        .get("session")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let objective = manifest
        .get("objective")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut matched =
        query.is_none_or(|q| id.to_lowercase().contains(q) || objective.to_lowercase().contains(q));
    let mut record = Record {
        id: id.into(),
        title: clean(objective, 100),
        kind: if session { "Session" } else { "Run" },
        status: "In progress or interrupted",
        started_at: started_at(id),
        turns: 0,
        requests: 0,
        cost_usd: 0.0,
        goal: None,
        previous_goals: Vec::new(),
        resume_command: None,
        incomplete: false,
        notes: Vec::new(),
        messages: Vec::new(),
    };
    match goals(ws, id, budget) {
        Ok((goal, previous)) => {
            record.goal = goal;
            record.previous_goals = previous;
        }
        Err(_) => record
            .notes
            .push("Saved goal details could not be read safely.".into()),
    }
    for goal in record.goal.iter().chain(&record.previous_goals) {
        matched |= query.is_some_and(|q| {
            goal.objective.to_lowercase().contains(q)
                || goal
                    .summary
                    .as_ref()
                    .is_some_and(|s| s.to_lowercase().contains(q))
        });
    }
    let (bytes, limited) = transcript(ws, id, MAX_TRANSCRIPT.min(*budget))?;
    *budget = budget.saturating_sub(bytes.len() as u64);
    let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    if limited || complete < bytes.len() {
        record.incomplete = true;
        record.notes.push(if limited { "Only the beginning of this large transcript was read; counts and cost are partial." } else { "The latest transcript entry is still being written or was interrupted; only complete entries are shown." }.into());
    }
    let mut has_items = false;
    let mut closed = false;
    let mut malformed = 0;
    for line in bytes[..complete]
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
    {
        let Ok(entry) = serde_json::from_slice::<Value>(line) else {
            malformed += 1;
            continue;
        };
        record.cost_usd += cost(&entry);
        match entry.get("kind").and_then(Value::as_str).unwrap_or("") {
            "start" if record.title.is_empty() => {
                record.title = clean(
                    entry.get("objective").and_then(Value::as_str).unwrap_or(""),
                    100,
                );
            }
            "turn_start" => {
                session = true;
                closed = false;
                record.turns = record
                    .turns
                    .max(entry.get("exchange").and_then(Value::as_u64).unwrap_or(0));
                record.status = "In progress or interrupted";
                message(
                    &mut record,
                    "You",
                    entry.get("message"),
                    detailed,
                    query,
                    &mut matched,
                );
            }
            "steered" => {
                if let Some(messages) = entry.get("messages").and_then(Value::as_array) {
                    for text in messages {
                        message(
                            &mut record,
                            "You",
                            Some(text),
                            detailed,
                            query,
                            &mut matched,
                        );
                    }
                }
            }
            "turn_end" => {
                let end = &entry["end"];
                record.status = match end["state"].as_str().unwrap_or("") {
                    "asked" => "Waiting for your reply",
                    "failed" => "Turn failed",
                    "budget_stopped" => "Budget limit reached",
                    "interrupted" | "stopped" => "Paused",
                    _ => "Ready to continue",
                };
                let text = end
                    .get("message")
                    .or_else(|| end.get("question"))
                    .or_else(|| end.get("summary"))
                    .or_else(|| end.get("reason"));
                message(&mut record, "Duet", text, detailed, query, &mut matched);
            }
            "usage" => record.requests += 1,
            "item" => {
                has_items = true;
                let item = &entry["item"];
                let speaker = match item["type"].as_str().unwrap_or("") {
                    "user" if !session => Some("You"),
                    "assistant" if !session => Some("Duet"),
                    _ => None,
                };
                if let Some(speaker) = speaker {
                    message(
                        &mut record,
                        speaker,
                        item.get("text"),
                        detailed,
                        query,
                        &mut matched,
                    );
                }
            }
            "end" => {
                let end = &entry["terminal"];
                closed = end["state"] == "completed";
                record.status = match end["state"].as_str().unwrap_or("") {
                    "completed" if session => "Closed",
                    "completed" => "Completed",
                    "open" => "Open",
                    "budget_stopped" => "Budget limit reached",
                    "failed"
                        if end["reason"].as_str() == Some(duet_agent::session::SESSION_LEFT) =>
                    {
                        "Open"
                    }
                    "failed"
                        if end["reason"]
                            .as_str()
                            .is_some_and(|s| s.contains("interrupted")) =>
                    {
                        "Interrupted"
                    }
                    "failed" => "Failed",
                    _ => "Unknown",
                };
                if !session {
                    message(
                        &mut record,
                        "Duet",
                        end.get("summary").or_else(|| end.get("reason")),
                        detailed,
                        query,
                        &mut matched,
                    );
                }
            }
            _ => {}
        }
    }
    if malformed > 0 {
        record.incomplete = true;
        record.notes.push(format!("Skipped {malformed} unreadable transcript entry/entries; counts and cost may be incomplete."));
    }
    if bytes.is_empty() {
        record.status = "No conversation recorded";
        record.notes.push("No saved messages yet.".into());
    }
    if limited {
        record.status = "Partial history";
    }
    if record.title.is_empty() {
        record.title = "Untitled conversation".into();
    }
    record.kind = if session { "Session" } else { "Run" };
    if !session {
        record.turns = record.requests;
    }
    if has_items && !closed && !limited {
        record.resume_command = Some(if session {
            format!("duet --resume {id}")
        } else {
            format!("duet resume {id}")
        });
    }
    matched |= query.is_some_and(|q| record.title.to_lowercase().contains(q));
    Ok((record, matched))
}

fn cost(entry: &Value) -> f64 {
    let value = match entry["kind"].as_str().unwrap_or("") {
        "usage" | "failed_attempts" => entry["cost_usd"].as_f64(),
        "review_usage" => entry["usage"]["cost_usd"].as_f64(),
        "subagent" => return cost(&entry["entry"]),
        _ => None,
    }
    .unwrap_or(0.0);
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

fn message(
    record: &mut Record,
    speaker: &'static str,
    value: Option<&Value>,
    detailed: bool,
    query: Option<&str>,
    matched: &mut bool,
) {
    let Some(text) = value
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    else {
        return;
    };
    *matched |= query.is_some_and(|q| text.to_lowercase().contains(q));
    if record.title.is_empty() && speaker == "You" {
        record.title = clean(text, 100);
    }
    if detailed {
        let text = text
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect::<String>();
        if !record
            .messages
            .last()
            .is_some_and(|m| m.speaker == speaker && m.text == text)
        {
            record.messages.push(Message { speaker, text });
        }
    }
}

fn clean(text: &str, limit: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = text.chars().filter(|c| !c.is_control());
    let mut out: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

fn started_at(id: &str) -> String {
    let pieces: Vec<_> = id.split('-').collect();
    if pieces.len() >= 2
        && pieces[0].len() == 8
        && pieces[1].len() == 6
        && pieces[..2]
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_digit()))
    {
        let date = pieces[0];
        let clock = pieces[1];
        return format!(
            "{}-{}-{} {}:{} UTC",
            &date[..4],
            &date[4..6],
            &date[6..],
            &clock[..2],
            &clock[2..4]
        );
    }
    "Time unavailable".into()
}

fn render_listing(list: &Listing, searched: bool) -> String {
    let mut out: String = if list.runs.is_empty() {
        if searched {
            "No matching conversations. Try different words.\n".into()
        } else {
            "No saved conversations in this workspace yet. Start one with duet.\n".into()
        }
    } else {
        "Recent conversations\n\n".into()
    };
    for (i, record) in list.runs.iter().enumerate() {
        out.push_str(&format!(
            "{}. {}\n   {} · {} · {} · {} turns · USD {:.4}{}\n   ID: {}\n",
            i + 1,
            record.title,
            record.kind,
            record.status,
            record.started_at,
            record.turns,
            record.cost_usd,
            if record.incomplete { " (partial)" } else { "" },
            record.id
        ));
        if let Some(command) = &record.resume_command {
            out.push_str(&format!("   Continue: {command}\n"));
        }
        if let Some(goal) = &record.goal {
            out.push_str(&format!(
                "   Goal: {} · {} of {} turns · {}\n",
                goal.status,
                goal.turns,
                goal.max_turns,
                clean(&goal.objective, 100)
            ));
        }
        out.push('\n');
    }
    for note in &list.notes {
        out.push_str(&format!("{note}\n"));
    }
    if !list.runs.is_empty() {
        out.push_str("Open a conversation: duet history <ID>\nFind a conversation: duet history --search <words>\nCosts are recorded frontier estimates; provider billing may differ.\n");
    }
    out.trim_end().to_owned()
}

fn detail(record: &Record) -> String {
    let mut out = format!(
        "{}\n{} · {} · {}\nID: {}\n{} turns · {} model requests · USD {:.4} recorded frontier cost{}\n",
        record.title,
        record.kind,
        record.status,
        record.started_at,
        record.id,
        record.turns,
        record.requests,
        record.cost_usd,
        if record.incomplete { " (partial)" } else { "" }
    );
    for note in &record.notes {
        out.push_str(&format!("{note}\n"));
    }
    if let Some(command) = &record.resume_command {
        out.push_str(&format!("Continue: {command}\n"));
    }
    if let Some(goal) = &record.goal {
        out.push_str(&format!(
            "\nGoal: {}\n{}\n{} of {} goal turns used\n",
            goal.status, goal.objective, goal.turns, goal.max_turns
        ));
        if let Some(summary) = &goal.summary {
            out.push_str(&format!("Progress: {summary}\n"));
        }
        if let Some(reason) = &goal.reason {
            out.push_str(&format!("{reason}\n"));
        }
        if goal.status == "Waiting for your answer" && record.resume_command.is_some() {
            out.push_str(
                "After continuing this session, answer Duet's question to continue the goal.\n",
            );
        } else if matches!(goal.status, "Paused" | "Working or interrupted")
            && record.resume_command.is_some()
        {
            out.push_str(
                "After continuing this session, use /goal resume to restart automatic work.\n",
            );
        }
    }
    if !record.previous_goals.is_empty() {
        out.push_str("\nPrevious goals\n");
        for (i, goal) in record.previous_goals.iter().enumerate() {
            out.push_str(&format!(
                "\n{}. {} · {} of {} turns\n{}\n",
                i + 1,
                goal.status,
                goal.turns,
                goal.max_turns,
                goal.objective
            ));
            if let Some(summary) = &goal.summary {
                out.push_str(&format!("Progress: {summary}\n"));
            }
            if let Some(reason) = &goal.reason {
                out.push_str(&format!("{reason}\n"));
            }
        }
    }
    for message in &record.messages {
        out.push_str(&format!("\n{}\n{}\n", message.speaker, message.text));
    }
    out.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn saved(ws: &Path, id: &str, manifest: Value, entries: &[Value]) -> PathBuf {
        let dir = ws.join(".duet/runs").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
        let log: String = entries.iter().map(|v| format!("{v}\n")).collect();
        std::fs::write(dir.join("transcript.jsonl"), log).unwrap();
        dir
    }

    fn read(ws: &Path, id: &str) -> Record {
        let mut budget = MAX_TRANSCRIPT;
        read_record(ws, id, true, None, &mut budget).unwrap().0
    }

    #[test]
    fn empty_history_is_helpful_and_creates_nothing() {
        let ws = tempfile::tempdir().unwrap();
        assert!(summary(ws.path()).unwrap().contains("Start one with duet"));
        assert!(!ws.path().join(".duet").exists());
    }

    #[test]
    fn open_sessions_and_exact_legacy_exits_are_resumable_without_hiding_real_failures() {
        let ws = tempfile::tempdir().unwrap();
        for (id, terminal, expected) in [
            (
                "open-session",
                json!({"state":"open","reason":duet_agent::session::SESSION_LEFT}),
                "Open",
            ),
            (
                "legacy-session",
                json!({"state":"failed","reason":duet_agent::session::SESSION_LEFT}),
                "Open",
            ),
            (
                "failed-session",
                json!({"state":"failed","reason":format!("save failed: {}", duet_agent::session::SESSION_LEFT)}),
                "Failed",
            ),
        ] {
            let dir = saved(
                ws.path(),
                id,
                json!({"session":true,"objective":"Test task"}),
                &[
                    json!({"kind":"turn_start","exchange":1,"message":"Test task"}),
                    json!({"kind":"item","item":{"type":"user","text":"Test task"}}),
                    json!({"kind":"turn_end","end":{"state":"interrupted"}}),
                    json!({"kind":"end","terminal":terminal}),
                ],
            );
            let before = std::fs::read(dir.join("transcript.jsonl")).unwrap();
            let record = read(ws.path(), id);
            assert_eq!(record.status, expected);
            assert_eq!(
                record.resume_command.as_deref(),
                Some(format!("duet --resume {id}").as_str())
            );
            assert_eq!(std::fs::read(dir.join("transcript.jsonl")).unwrap(), before);
        }
    }

    #[test]
    fn legacy_runs_show_conversation_cost_and_resume_command() {
        let ws = tempfile::tempdir().unwrap();
        saved(
            ws.path(),
            "legacy-run",
            json!({"objective":"Fix the parser"}),
            &[
                json!({"kind":"item","item":{"type":"user","text":"Fix the parser"}}),
                json!({"kind":"item","item":{"type":"assistant","text":"I found the issue."}}),
                json!({"kind":"usage","turn":1,"cost_usd":0.25}),
                json!({"kind":"subagent","entry":{"kind":"usage","cost_usd":0.1}}),
                json!({"kind":"subagent_end","cost_usd":0.1}),
                json!({"kind":"review_usage","usage":{"cost_usd":0.05}}),
            ],
        );
        let record = read(ws.path(), "legacy-run");
        assert_eq!(record.kind, "Run");
        assert_eq!(record.turns, 1);
        assert!((record.cost_usd - 0.4).abs() < 1e-12);
        assert_eq!(
            record.resume_command.as_deref(),
            Some("duet resume legacy-run")
        );
        assert!(detail(&record).contains("I found the issue."));
    }

    #[test]
    fn incomplete_active_tail_is_never_repaired_or_truncated() {
        use std::io::Write;
        let ws = tempfile::tempdir().unwrap();
        let dir = saved(
            ws.path(),
            "20261001-163419-abc123",
            json!({"session":true,"objective":"Make setup easier"}),
            &[
                json!({"kind":"turn_start","exchange":1,"message":"Make setup easier"}),
                json!({"kind":"item","item":{"type":"user","text":"Make setup easier"}}),
            ],
        );
        let path = dir.join("transcript.jsonl");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"kind\":\"usage\"")
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        let record = read(ws.path(), "20261001-163419-abc123");
        assert!(record.incomplete);
        assert_eq!(record.turns, 1);
        assert_eq!(record.started_at, "2026-10-01 16:34 UTC");
        assert_eq!(record.messages.len(), 1);
        assert_eq!(
            record.resume_command.as_deref(),
            Some("duet --resume 20261001-163419-abc123")
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn search_finds_messages_and_closed_sessions_have_no_resume_action() {
        let ws = tempfile::tempdir().unwrap();
        saved(
            ws.path(),
            "20261001-120000-abc123",
            json!({"session":true,"objective":"Website"}),
            &[
                json!({"kind":"turn_start","exchange":1,"message":"Use a violet header"}),
                json!({"kind":"item","item":{"type":"user","text":"Use a violet header"}}),
                json!({"kind":"turn_end","end":{"state":"replied","message":"Done."}}),
                json!({"kind":"end","terminal":{"state":"completed","summary":"Closed"}}),
            ],
        );
        let found = list(ws.path(), 20, Some("VIOLET")).unwrap();
        assert_eq!(found.runs.len(), 1);
        assert_eq!(found.runs[0].status, "Closed");
        assert!(found.runs[0].resume_command.is_none());
        assert!(list(ws.path(), 20, Some("orange")).unwrap().runs.is_empty());
    }

    #[test]
    fn live_goal_status_is_read_without_pausing_or_rewriting_it() {
        let ws = tempfile::tempdir().unwrap();
        let dir = saved(
            ws.path(),
            "goal-session",
            json!({"session":true,"objective":"Original task"}),
            &[json!({"kind":"item","item":{"type":"user","text":"Original task"}})],
        );
        let path = dir.join("goals.json");
        let bytes = serde_json::to_vec(&json!({
            "version":1, "history":[{
                "objective":"Fix earlier authentication","state":"completed",
                "turns":3,"max_turns":20,"summary":"Verified login flow","reason":null
            }],
            "current":{"objective":"Improve model discovery","state":"active",
                "turns":2,"max_turns":20,"summary":"Found three providers","reason":null}
        }))
        .unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let record = read(ws.path(), "goal-session");
        assert_eq!(
            record.goal.as_ref().unwrap().status,
            "Working or interrupted"
        );
        assert!(detail(&record).contains("Found three providers"));
        assert!(detail(&record).contains("Previous goals"));
        assert!(detail(&record).contains("Verified login flow"));
        assert_eq!(record.previous_goals.len(), 1);
        let encoded = render(
            ws.path(),
            Args {
                id: Some("goal-session".into()),
                limit: 20,
                search: None,
                json: true,
            },
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&encoded).unwrap()["previous_goals"][0]["status"],
            "Completed"
        );
        assert_eq!(
            list(ws.path(), 20, Some("discovery")).unwrap().runs.len(),
            1
        );
        assert_eq!(
            list(ws.path(), 20, Some("authentication"))
                .unwrap()
                .runs
                .len(),
            1
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn unsafe_ids_and_symlinked_artifacts_are_rejected() {
        use std::os::unix::fs::symlink;
        let ws = tempfile::tempdir().unwrap();
        for id in [
            "../elsewhere",
            "/tmp/elsewhere",
            "x; touch bad",
            "x\n",
            "..",
            "",
        ] {
            let mut budget = MAX_TRANSCRIPT;
            assert!(read_record(ws.path(), id, true, None, &mut budget).is_err());
        }
        let target = tempfile::tempdir().unwrap();
        let dir = saved(ws.path(), "safe", json!({"objective":"Safe"}), &[]);
        let path = dir.join("transcript.jsonl");
        std::fs::remove_file(&path).unwrap();
        std::fs::write(target.path().join("private"), "secret").unwrap();
        symlink(target.path().join("private"), &path).unwrap();
        let mut budget = MAX_TRANSCRIPT;
        assert!(read_record(ws.path(), "safe", true, None, &mut budget).is_err());
        std::fs::remove_dir_all(ws.path().join(".duet")).unwrap();
        symlink(target.path(), ws.path().join(".duet")).unwrap();
        assert!(list(ws.path(), 20, None).is_err());
    }

    #[test]
    fn bounded_reads_report_partial_metrics_without_changing_the_file() {
        let ws = tempfile::tempdir().unwrap();
        let dir = saved(
            ws.path(),
            "bounded",
            json!({"objective":"Long task"}),
            &[
                json!({"kind":"item","item":{"type":"user","text":"Long task"}}),
                json!({"kind":"usage","cost_usd":0.1}),
            ],
        );
        let before = std::fs::read(dir.join("transcript.jsonl")).unwrap();
        let mut budget = 40;
        let (record, _) = read_record(ws.path(), "bounded", true, None, &mut budget).unwrap();
        assert!(record.incomplete);
        assert_eq!(record.status, "Partial history");
        assert!(record.resume_command.is_none());
        assert_eq!(budget, 0);
        assert_eq!(std::fs::read(dir.join("transcript.jsonl")).unwrap(), before);
    }
}
