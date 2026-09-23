// SPDX-License-Identifier: GPL-3.0-or-later
//! The frontier's tools. The set is fixed for a run and sent sorted by name,
//! so the request prefix never changes.

use crate::journal::WriteJournal;
use duet_boundary::model::ToolSpec;
use duet_boundary::view::{Presenter, Source};
use duet_fs::{FsError, Precondition};
use duet_git::Git;
use duet_sandbox::{SandboxKind, Spec};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LIST: usize = 2000;
const MAX_MATCHES: usize = 200;

/// Everything a tool may touch.
pub struct Ctx<'a> {
    pub workspace: &'a Path,
    pub run_dir: &'a Path,
    pub sandbox: SandboxKind,
    pub git: &'a Git,
    pub presenter: &'a dyn Presenter,
    pub journal: &'a mut WriteJournal,
    pub command_timeout: Duration,
    pub network: bool,
    pub checks: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Content for the tool result.
    Result(String),
    /// A tool error; the frontier sees the message and can adjust.
    Error(String),
    /// `finish` was called and every check passed.
    Finished { summary: String },
    /// `finish` was called but checks failed; their output is shown.
    ChecksFailed(String),
}

fn string_arg<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string argument `{key}`"))
}

fn fs_err(e: FsError) -> String {
    e.to_string()
}

/// The tool specifications (built-in plus the presenter's), sorted by name.
pub fn specs_with(extra: Vec<ToolSpec>) -> Vec<ToolSpec> {
    let mut all = specs();
    all.extend(extra);
    all.sort_by(|a, b| a.name.cmp(&b.name));
    all
}

/// The built-in tool specifications, sorted by name.
pub fn specs() -> Vec<ToolSpec> {
    let t = |name: &str, description: &str, parameters: Value| ToolSpec {
        name: name.to_owned(),
        description: description.to_owned(),
        parameters,
    };
    let mut specs = vec![
        t(
            "read_file",
            "Read a text file. Lines are numbered. Use start_line/end_line (1-based, inclusive) for large files.",
            json!({"type": "object", "properties": {
                "path": {"type": "string", "description": "Path relative to the repository root."},
                "start_line": {"type": "integer", "minimum": 1},
                "end_line": {"type": "integer", "minimum": 1}
            }, "required": ["path"]}),
        ),
        t(
            "list_files",
            "List repository files (respecting .gitignore). Optionally restrict to a directory.",
            json!({"type": "object", "properties": {
                "dir": {"type": "string", "description": "Directory relative to the root; default is the whole repository."}
            }}),
        ),
        t(
            "search",
            "Search file contents with a regular expression. Returns path:line: text for each match.",
            json!({"type": "object", "properties": {
                "pattern": {"type": "string"},
                "dir": {"type": "string", "description": "Limit the search to this directory."}
            }, "required": ["pattern"]}),
        ),
        t(
            "diff",
            "Show your changes so far (working tree compared with the starting commit).",
            json!({"type": "object", "properties": {}}),
        ),
        t(
            "edit_file",
            "Edit a file by exact text replacement. Each `old` must occur exactly once in the file. \
All edits are applied together or not at all.",
            json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "edits": {"type": "array", "minItems": 1, "items": {"type": "object", "properties": {
                    "old": {"type": "string", "description": "Exact existing text, including indentation."},
                    "new": {"type": "string"}
                }, "required": ["old", "new"]}}
            }, "required": ["path", "edits"]}),
        ),
        t(
            "write_file",
            "Create a file or replace its whole content.",
            json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"}
            }, "required": ["path", "content"]}),
        ),
        t(
            "run_command",
            "Run a shell command in the repository root (sandboxed: no network, writes limited to the repository). \
Returns the exit code and output.",
            json!({"type": "object", "properties": {
                "command": {"type": "string"},
                "timeout_seconds": {"type": "integer", "minimum": 1}
            }, "required": ["command"]}),
        ),
        t(
            "finish",
            "Declare the task complete. The host then runs the configured checks.",
            json!({"type": "object", "properties": {
                "summary": {"type": "string", "description": "What you changed and why."}
            }, "required": ["summary"]}),
        ),
    ];
    specs.sort_by(|a, b| a.name.cmp(&b.name));
    specs
}

pub async fn dispatch(ctx: &mut Ctx<'_>, name: &str, args: &Map<String, Value>) -> Outcome {
    let result = match name {
        "read_file" => read_file(ctx, args),
        "list_files" => list_files(ctx, args),
        "search" => search(ctx, args),
        "diff" => diff(ctx),
        "edit_file" => edit_file(ctx, args),
        "write_file" => write_file(ctx, args),
        "run_command" => run_command(ctx, args).await,
        "finish" => return finish(ctx, args).await,
        other => match ctx.presenter.call_tool(other, args) {
            Some(r) => r,
            None => Err(format!("unknown tool `{other}`")),
        },
    };
    match result {
        Ok(text) => Outcome::Result(text),
        Err(e) => Outcome::Error(e),
    }
}

fn read_file(ctx: &Ctx<'_>, args: &Map<String, Value>) -> Result<String, String> {
    let raw = string_arg(args, "path")?;
    let rel = duet_fs::normalize_relative(raw).map_err(fs_err)?;
    if !ctx.presenter.path_visible(&rel) {
        return Err(format!("{raw} is not available"));
    }
    let bytes = duet_fs::read_file(ctx.workspace, &rel, MAX_READ_BYTES).map_err(fs_err)?;
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let start = args
        .get("start_line")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let end = args
        .get("end_line")
        .and_then(Value::as_u64)
        .map_or(total, |e| (e as usize).min(total));
    if start > end.max(1) && total > 0 {
        return Err(format!(
            "line range {start}-{end} is outside the file ({total} lines)"
        ));
    }
    let width = end.to_string().len();
    let mut numbered = String::new();
    for (i, line) in lines.iter().enumerate().take(end).skip(start - 1) {
        numbered.push_str(&format!("{:>width$}  {line}\n", i + 1));
    }
    let header = format!(
        "{} (lines {}-{} of {total})\n",
        rel.display(),
        start.min(total.max(1)),
        end
    );
    let body = ctx
        .presenter
        .present(&Source::File { path: rel }, numbered.as_bytes());
    Ok(format!("{header}{body}"))
}

fn visible_files(ctx: &Ctx<'_>, dir: Option<&str>) -> Result<Vec<String>, String> {
    let prefix = match dir {
        Some(d) if !d.trim().is_empty() && d.trim() != "." => {
            let rel = duet_fs::normalize_relative(d).map_err(fs_err)?;
            Some(format!("{}/", rel.display()))
        }
        _ => None,
    };
    let files = ctx
        .git
        .list_files(ctx.workspace)
        .map_err(|e| e.to_string())?;
    Ok(files
        .into_iter()
        .filter(|f| prefix.as_ref().is_none_or(|p| f.starts_with(p.as_str())))
        .filter(|f| ctx.presenter.path_visible(Path::new(f)))
        .collect())
}

fn list_files(ctx: &Ctx<'_>, args: &Map<String, Value>) -> Result<String, String> {
    let files = visible_files(ctx, args.get("dir").and_then(Value::as_str))?;
    let total = files.len();
    let mut out: String = files
        .iter()
        .take(MAX_LIST)
        .map(|f| format!("{f}\n"))
        .collect();
    if total > MAX_LIST {
        out.push_str(&format!(
            "[{} more files not shown; narrow with `dir`]\n",
            total - MAX_LIST
        ));
    }
    Ok(ctx.presenter.present(&Source::FileList, out.as_bytes()))
}

fn search(ctx: &Ctx<'_>, args: &Map<String, Value>) -> Result<String, String> {
    let pattern = string_arg(args, "pattern")?;
    let re = Regex::new(pattern).map_err(|e| format!("invalid regular expression: {e}"))?;
    let mut out = String::new();
    let mut count = 0;
    for f in visible_files(ctx, args.get("dir").and_then(Value::as_str))? {
        let Ok(bytes) = duet_fs::read_file(ctx.workspace, Path::new(&f), MAX_READ_BYTES) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        for (i, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
            if re.is_match(line) {
                count += 1;
                if count <= MAX_MATCHES {
                    let shown: String = line.chars().take(300).collect();
                    out.push_str(&format!("{f}:{}: {shown}\n", i + 1));
                }
            }
        }
    }
    if count == 0 {
        return Ok("no matches".to_owned());
    }
    if count > MAX_MATCHES {
        out.push_str(&format!(
            "[{} more matches not shown]\n",
            count - MAX_MATCHES
        ));
    }
    Ok(ctx.presenter.present(
        &Source::Search {
            pattern: pattern.to_owned(),
        },
        out.as_bytes(),
    ))
}

fn diff(ctx: &Ctx<'_>) -> Result<String, String> {
    let files: Vec<String> = ctx
        .git
        .list_files(ctx.workspace)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|f| ctx.presenter.path_visible(Path::new(f)))
        .collect();
    let mut text = ctx
        .git
        .diff_paths(ctx.workspace, &files)
        .map_err(|e| e.to_string())?;
    let untracked = ctx
        .git
        .run(
            ctx.workspace,
            &["ls-files", "--others", "--exclude-standard"],
            &[],
            None,
        )
        .map(|o| String::from_utf8_lossy(&o).into_owned())
        .unwrap_or_default();
    let new_files: Vec<&str> = untracked
        .lines()
        .filter(|f| ctx.presenter.path_visible(Path::new(f)))
        .collect();
    if !new_files.is_empty() {
        text.push_str(&format!("\nNew files: {}\n", new_files.join(", ")));
    }
    if text.trim().is_empty() {
        return Ok("no changes".to_owned());
    }
    Ok(ctx.presenter.present(&Source::Diff, text.as_bytes()))
}

#[cfg(test)]
fn apply_edits(original: &str, edits: &[Value]) -> Result<String, String> {
    apply_edits_with(original, edits, &|s| s.to_owned())
}

/// Applies edits; `unmask` turns placeholders in `old` anchors back into real text.
fn apply_edits_with(
    original: &str,
    edits: &[Value],
    unmask: &dyn Fn(&str) -> String,
) -> Result<String, String> {
    let mut text = original.to_owned();
    for (i, e) in edits.iter().enumerate() {
        let old = e
            .get("old")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("edit {i}: missing `old`"))?;
        let old = &unmask(old);
        let new = e
            .get("new")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("edit {i}: missing `new`"))?;
        if old.is_empty() {
            return Err(format!("edit {i}: `old` must not be empty"));
        }
        match text.matches(old).count() {
            1 => text = text.replacen(old, new, 1),
            0 => {
                let hint = closest_line(&text, old);
                return Err(format!(
                    "edit {i}: `old` text not found; no edits applied.{hint}"
                ));
            }
            n => {
                return Err(format!(
                    "edit {i}: `old` text occurs {n} times; include more surrounding lines. No edits applied."
                ));
            }
        }
    }
    Ok(text)
}

/// Points at the most similar line to help the model correct a stale anchor.
fn closest_line(text: &str, needle: &str) -> String {
    let first = needle
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if first.is_empty() {
        return String::new();
    }
    text.lines()
        .enumerate()
        .find(|(_, l)| l.trim() == first)
        .map(|(i, _)| {
            format!(
                " The first line of `old` appears at line {}; re-read the file around it.",
                i + 1
            )
        })
        .unwrap_or_default()
}

fn edit_file(ctx: &mut Ctx<'_>, args: &Map<String, Value>) -> Result<String, String> {
    let raw = string_arg(args, "path")?;
    let rel = duet_fs::writable_relative(raw).map_err(fs_err)?;
    let edits = args
        .get("edits")
        .and_then(Value::as_array)
        .ok_or("missing array argument `edits`")?;
    let bytes = duet_fs::read_file(ctx.workspace, &rel, MAX_READ_BYTES).map_err(fs_err)?;
    let original =
        String::from_utf8(bytes.clone()).map_err(|_| "file is not valid UTF-8".to_owned())?;
    let presenter = ctx.presenter;
    let updated = apply_edits_with(&original, edits, &|s| presenter.detokenize(s))?;
    // Placeholders in the new text are resolved locally (and checked against secret sinks).
    let updated = presenter.resolve_for_write(&rel, &updated)?;
    let pre = Precondition::Sha256(duet_fs::sha256_hex(&bytes));
    ctx.journal
        .write(ctx.workspace, &rel, updated.as_bytes(), &pre)
        .map_err(fs_err)?;
    Ok(format!(
        "edited {} ({} edit{})",
        rel.display(),
        edits.len(),
        if edits.len() == 1 { "" } else { "s" }
    ))
}

fn write_file(ctx: &mut Ctx<'_>, args: &Map<String, Value>) -> Result<String, String> {
    let raw = string_arg(args, "path")?;
    let content = string_arg(args, "content")?;
    let rel = duet_fs::writable_relative(raw).map_err(fs_err)?;
    let existed = duet_fs::read_optional(ctx.workspace, &rel, MAX_READ_BYTES)
        .map_err(fs_err)?
        .is_some();
    let content = ctx.presenter.resolve_for_write(&rel, content)?;
    ctx.journal
        .write(ctx.workspace, &rel, content.as_bytes(), &Precondition::Any)
        .map_err(fs_err)?;
    Ok(format!(
        "{} {} ({} bytes)",
        if existed { "replaced" } else { "created" },
        rel.display(),
        content.len()
    ))
}

async fn sandboxed(
    ctx: &Ctx<'_>,
    command: &str,
    timeout: Duration,
) -> Result<duet_sandbox::Output, String> {
    let spec = Spec {
        workspace: ctx.workspace.to_path_buf(),
        scratch: ctx.workspace.join(".duet/tmp"),
        network: ctx.network,
        timeout,
        output_cap: 256 * 1024,
        spill_file: Some(
            ctx.run_dir
                .join(format!("spill-{}.txt", uuid::Uuid::new_v4())),
        ),
        extra_env: vec![
            ("CARGO_TERM_COLOR".into(), "never".into()),
            ("NO_COLOR".into(), "1".into()),
        ],
    };
    duet_sandbox::run(
        ctx.sandbox,
        &spec,
        &["/bin/sh".into(), "-c".into(), command.into()],
        ctx.workspace,
    )
    .await
    .map_err(|e| e.to_string())
}

fn render_output(o: &duet_sandbox::Output) -> Vec<u8> {
    let mut text = match (o.timed_out, o.exit_code) {
        (true, _) => format!("timed out after {:.0}s\n", o.duration.as_secs_f64()),
        (false, Some(c)) => format!("exit code {c}\n"),
        (false, None) => "terminated by a signal\n".to_owned(),
    };
    for (label, bytes, total) in [
        ("stdout", &o.stdout, o.stdout_total),
        ("stderr", &o.stderr, o.stderr_total),
    ] {
        if total > 0 {
            text.push_str(&format!(
                "--- {label} ---\n{}",
                String::from_utf8_lossy(bytes)
            ));
            if total > bytes.len() {
                text.push_str(&format!(
                    "\n[{label} truncated: {} of {total} bytes shown]",
                    bytes.len()
                ));
            }
            text.push('\n');
        }
    }
    text.into_bytes()
}

async fn run_command(ctx: &mut Ctx<'_>, args: &Map<String, Value>) -> Result<String, String> {
    let command = string_arg(args, "command")?;
    let timeout = args
        .get("timeout_seconds")
        .and_then(Value::as_u64)
        .map_or(ctx.command_timeout, Duration::from_secs)
        .min(ctx.command_timeout);
    let o = sandboxed(ctx, command, timeout).await?;
    let source = Source::Command {
        command: command.to_owned(),
        exit_code: o.exit_code,
    };
    Ok(ctx.presenter.present(&source, &render_output(&o)))
}

async fn finish(ctx: &mut Ctx<'_>, args: &Map<String, Value>) -> Outcome {
    let summary = args
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mut report = Vec::new();
    let mut failed = false;
    for check in ctx.checks {
        match sandboxed(ctx, check, ctx.command_timeout).await {
            Ok(o) => {
                let passed = o.exit_code == Some(0) && !o.timed_out;
                failed |= !passed;
                report.extend_from_slice(format!("$ {check}\n").as_bytes());
                report.extend(render_output(&o));
            }
            Err(e) => {
                failed = true;
                report.extend_from_slice(format!("$ {check}\ncould not run: {e}\n").as_bytes());
            }
        }
    }
    if failed {
        Outcome::ChecksFailed(ctx.presenter.present(&Source::Checks, &report))
    } else {
        Outcome::Finished { summary }
    }
}

/// Paths the run wrote (for reports).
pub fn written_paths(journal: &WriteJournal) -> Vec<PathBuf> {
    journal.paths()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_are_sorted_and_stable() {
        let a =
            serde_json::to_string(&specs().iter().map(|s| &s.name).collect::<Vec<_>>()).unwrap();
        let b =
            serde_json::to_string(&specs().iter().map(|s| &s.name).collect::<Vec<_>>()).unwrap();
        assert_eq!(a, b);
        let names: Vec<_> = specs().into_iter().map(|s| s.name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn edits_apply_all_or_nothing() {
        let text = "fn a() {}\nfn b() {}\n";
        let ok = apply_edits(
            text,
            &[
                json!({"old": "fn a()", "new": "fn a2()"}),
                json!({"old": "fn b()", "new": "fn b2()"}),
            ],
        )
        .unwrap();
        assert_eq!(ok, "fn a2() {}\nfn b2() {}\n");
        let missing = apply_edits(
            text,
            &[
                json!({"old": "fn a()", "new": "x"}),
                json!({"old": "nope", "new": "y"}),
            ],
        );
        assert!(missing.unwrap_err().contains("not found"));
        let dup = apply_edits("x\nx\n", &[json!({"old": "x", "new": "y"})]);
        assert!(dup.unwrap_err().contains("2 times"));
    }
}
