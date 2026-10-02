// SPDX-License-Identifier: GPL-3.0-or-later
//! The frontier's tools. The set is fixed for a run and sent sorted by name,
//! so the request prefix never changes.

use crate::journal::WriteJournal;
use duet_boundary::audit::{AuditEvent, AuditHandle};
use duet_boundary::model::ToolSpec;
use duet_boundary::view::{Presenter, Source};
use duet_fs::{FsError, Precondition};
use duet_git::Git;
use duet_sandbox::{SandboxKind, Spec};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub(crate) const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;
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
    /// The run's `sandbox.network` (see [`crate::egress`] for who gets it).
    pub network: &'a crate::egress::Network,
    pub checks: &'a [String],
    /// The run's audit log, for security events (sandbox denials, sensitive commands).
    pub audit: Option<&'a AuditHandle>,
    /// The run's interrupt flag: when it is raised, a running command's
    /// process tree is killed at once.
    pub interrupted: Option<&'a AtomicBool>,
    /// Host-side web access (`web_fetch`, `web_search`); `None` when off.
    pub web: Option<&'a duet_web::Web>,
    /// The run's git tools (`None`: not a git repository, none offered).
    pub git_tools: Option<&'a crate::git_tools::GitTools>,
    /// The run's language servers (`code_nav`, `rename`); `None` when off.
    pub lsp: Option<&'a duet_lsp::Lsp>,
}

impl Ctx<'_> {
    pub(crate) fn record(&self, event: AuditEvent) {
        if let Some(a) = self.audit {
            a.record(event);
        }
    }
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

pub(crate) fn string_arg<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string argument `{key}`"))
}

pub(crate) fn fs_err(e: FsError) -> String {
    e.to_string()
}

/// The tool specifications (built-in plus the presenter's), sorted by name.
/// Built-in tools plus the presenter's; a presenter tool replaces a built-in of the same name.
pub fn specs_with(extra: Vec<ToolSpec>) -> Vec<ToolSpec> {
    let mut all = specs();
    all.retain(|s| !extra.iter().any(|e| e.name == s.name));
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
            "Read a text file. Lines are numbered. Use start_line/end_line (1-based, inclusive) for large files. \
An image (.png, .jpg, .jpeg, .gif, .webp) is returned as the image itself or, where images may not leave \
this machine, as a description by the local model.",
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
            "Show your changes so far: the working tree compared with the starting commit (outside a git \
repository: the files you wrote, compared with their content before the run).",
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
            &run_command_description(&crate::egress::Network::Off),
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

/// `run_command`'s description for a run with `network`.
pub(crate) fn run_command_description(network: &crate::egress::Network) -> String {
    format!(
        "Run a shell command in the repository root (sandboxed: {}, writes limited to the repository). \
Returns the exit code and output.",
        network.describe()
    )
}

pub async fn dispatch(ctx: &mut Ctx<'_>, name: &str, args: &Map<String, Value>) -> Outcome {
    let result = match name {
        "read_file" => read_file(ctx, args),
        "list_files" => list_files(ctx, args),
        "search" => search(ctx, args),
        "diff" => diff(ctx),
        "edit_file" | "write_file" => {
            let written = if name == "edit_file" {
                edit_file(ctx, args)
            } else {
                write_file(ctx, args)
            };
            match written {
                Ok(text) => Ok(crate::code_nav::with_diagnostics(ctx, args, text).await),
                Err(e) => Err(e),
            }
        }
        "run_command" => run_command(ctx, args).await,
        "finish" => return finish(ctx, args).await,
        "edit_protected" => crate::protected::edit_protected(ctx, args).await,
        "code_nav" if ctx.lsp.is_some() => crate::code_nav::code_nav(ctx, args).await,
        "rename" if ctx.lsp.is_some() => crate::code_nav::rename(ctx, args).await,
        crate::web::FETCH | crate::web::SEARCH if ctx.web.is_some() => {
            match crate::web::call(ctx, name, args).await {
                Some(r) => r,
                None => Err(format!("unknown tool `{name}`")),
            }
        }
        git if crate::git_tools::NAMES.contains(&git) => crate::git_tools::dispatch(ctx, git, args),
        other => match ctx.presenter.call_tool(other, args) {
            Some(r) => r,
            None => Err(format!("unknown tool `{other}`")),
        },
    };
    // What the presenter decided while the tool ran (a probe of a value,
    // a short sensitive output, a sample) goes to the audit log.
    for event in ctx.presenter.take_events() {
        ctx.record(event);
    }
    match result {
        Ok(text) => Outcome::Result(text),
        Err(e) => Outcome::Error(e),
    }
}

fn read_file(ctx: &Ctx<'_>, args: &Map<String, Value>) -> Result<String, String> {
    let raw = string_arg(args, "path")?;
    let rel = duet_fs::normalize_relative(raw).map_err(fs_err)?;
    // `.duet` (raw handles, the vault, transcripts) and `.git` (committed
    // copies) are run state, never workspace content.
    if duet_fs::is_reserved(&rel) || !ctx.presenter.path_visible(&rel) {
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
    let body = ctx.presenter.present(
        &Source::File {
            path: rel,
            ranged: args.contains_key("start_line") || args.contains_key("end_line"),
        },
        numbered.as_bytes(),
    );
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
    if !ctx.git.is_repository(ctx.workspace) {
        return Ok(written_diff(ctx));
    }
    // Sensitive files are named, never diffed for the frontier.
    let (sensitive, files): (Vec<String>, Vec<String>) = ctx
        .git
        .list_files(ctx.workspace)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|f| ctx.presenter.path_visible(Path::new(f)))
        .partition(|f| ctx.presenter.path_sensitive(Path::new(f)));
    // Against the run's base: its own commits (git_commit) stay in the diff.
    let base = crate::git_tools::diff_base(ctx);
    let mut text = ctx
        .git
        .diff_paths_from(ctx.workspace, &base, &files)
        .map_err(|e| e.to_string())?;
    let changed_sensitive = changed_names(ctx, &base, &sensitive);
    if !changed_sensitive.is_empty() {
        text.push_str(&format!(
            "\nChanged sensitive files (content held locally): {}\n",
            changed_sensitive.join(", ")
        ));
    }
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
        .filter(|f| !duet_fs::is_reserved(Path::new(f)) && ctx.presenter.path_visible(Path::new(f)))
        .collect();
    if !new_files.is_empty() {
        text.push_str(&format!("\nNew files: {}\n", new_files.join(", ")));
    }
    if text.trim().is_empty() {
        return Ok("no changes".to_owned());
    }
    Ok(ctx.presenter.present(&Source::Diff, text.as_bytes()))
}

/// Which of `paths` differ from `base` (names only; nothing is read out).
fn changed_names(ctx: &Ctx<'_>, base: &str, paths: &[String]) -> Vec<String> {
    if paths.is_empty() {
        return Vec::new();
    }
    let mut args = vec![
        "diff",
        "--name-only",
        "--no-ext-diff",
        "--no-textconv",
        "--end-of-options",
        base,
        "--",
    ];
    args.extend(paths.iter().map(String::as_str));
    ctx.git
        .run(ctx.workspace, &args, &[], None)
        .map(|o| {
            String::from_utf8_lossy(&o)
                .lines()
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// `diff` outside a git repository: the files this run wrote with its
/// tools, against their content before its first write (see
/// [`crate::changes`]). Sensitive files are named, never shown.
fn written_diff(ctx: &Ctx<'_>) -> String {
    let mut text = String::new();
    let mut sensitive = Vec::new();
    for c in crate::changes::written(ctx.git, ctx.workspace, ctx.run_dir) {
        if !ctx.presenter.path_visible(&c.path) {
            continue;
        }
        if ctx.presenter.path_sensitive(&c.path) {
            sensitive.push(c.path.display().to_string());
        } else {
            text.push_str(&c.diff);
        }
    }
    if !sensitive.is_empty() {
        text.push_str(&format!(
            "\nChanged sensitive files (content held locally): {}\n",
            sensitive.join(", ")
        ));
    }
    if text.trim().is_empty() {
        return format!("no changes\n{}", crate::changes::NOTE);
    }
    format!(
        "{}\n{}",
        crate::changes::NOTE,
        ctx.presenter.present(&Source::Diff, text.as_bytes())
    )
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
    for edit in edits {
        if let Some(new) = edit.get("new").and_then(Value::as_str) {
            presenter.note_authored(new);
        }
    }
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
    ctx.presenter.note_authored(content);
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

/// What a sandboxed command may read of the paths the presenter protects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// Nothing the presenter hides (enforced by the kernel).
    Ordinary,
    /// The host's checks: everything except sensitive data (protected source
    /// must compile); their output is presented as check output.
    Checks,
    /// Everything except Duet's run state (denied to every command); the
    /// output is held locally and placeholders in the command are resolved.
    SensitiveData,
    /// What an ordinary command may read, with the workspace read-only:
    /// writes only to the scratch directory (sub-agents' commands).
    ReadOnly,
}

/// Where sandboxed processes get their `TMPDIR`: outside the workspace, as
/// `.duet/` (run state) is unreadable and unwritable to them.
pub(crate) fn scratch_root() -> PathBuf {
    std::env::temp_dir().join("duet-scratch")
}

/// The run's name in scratch paths.
pub(crate) fn run_name(run_dir: &Path) -> String {
    run_dir
        .file_name()
        .map_or_else(|| "run".into(), |n| n.to_string_lossy().into_owned())
}

/// The `TMPDIR` of the run's `sensitive_data` commands. What they leave there
/// is derived data, like the files they write, so every other sandboxed
/// process denies it ([`hidden_from_processes`]). Created here, so a deny rule
/// covers it as a directory even for a process started before the first
/// sensitive command.
pub(crate) fn sensitive_scratch(run_dir: &Path) -> PathBuf {
    let dir = scratch_root().join(format!("{}-sensitive", run_name(run_dir)));
    if std::fs::create_dir_all(&dir).is_ok() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    dir
}

/// What a sandboxed process other than a `sensitive_data` command or a check
/// (an ordinary command, an MCP or language server) may not read: the
/// presenter's hidden paths and the sensitive commands' `TMPDIR`. Duet's run
/// state is denied by the sandbox itself, to every process.
pub(crate) fn hidden_from_processes(
    presenter: &dyn Presenter,
    workspace: &Path,
    run_dir: &Path,
) -> Vec<PathBuf> {
    let mut hidden = presenter.hidden_from_commands(workspace);
    hidden.push(sensitive_scratch(run_dir));
    hidden
}

/// Runs `command` in the sandbox with the given `access`.
pub(crate) async fn sandboxed(
    ctx: &Ctx<'_>,
    command: &str,
    timeout: Duration,
    access: Access,
) -> Result<duet_sandbox::Output, String> {
    let access_name = match access {
        Access::Ordinary => "ordinary",
        Access::Checks => "checks",
        Access::SensitiveData => "sensitive_data",
        Access::ReadOnly => "read_only",
    };
    // Checks may read protected source; then they get no network.
    let mut reads_protected = false;
    let mut deny_read = match access {
        Access::Ordinary | Access::ReadOnly => {
            hidden_from_processes(ctx.presenter, ctx.workspace, ctx.run_dir)
        }
        Access::Checks => {
            let mut hidden = ctx.presenter.hidden_from_checks(ctx.workspace);
            reads_protected =
                hidden.len() < ctx.presenter.hidden_from_commands(ctx.workspace).len();
            hidden.push(sensitive_scratch(ctx.run_dir));
            hidden
        }
        // Everything but run state, which the sandbox denies to every command.
        Access::SensitiveData => Vec::new(),
    };
    // Credential stores in the operator's home: never readable by a command.
    deny_read.extend(duet_sandbox::home_secrets(ctx.workspace));
    let scratch = match access {
        Access::SensitiveData => sensitive_scratch(ctx.run_dir),
        _ => scratch_root().join(run_name(ctx.run_dir)),
    };
    let mut note = None;
    let guard = ctx.presenter.outbound_guard();
    // A command the frontier wrote that would have network is text for a
    // third party: one that holds a placeholder or a withheld value (in any
    // spelling the check reads) runs without network. (What a command sends
    // inside a TLS tunnel is not visible to the proxy.)
    let offline = matches!(access, Access::Ordinary | Access::ReadOnly)
        && !matches!(ctx.network, crate::egress::Network::Off)
        && match guard.check_text("run_command", "the network", command) {
            Ok(()) => false,
            Err(r) => {
                note = Some(format!(
                    "\n[sandbox] the command ran without network: {}\n",
                    r.reason
                ));
                true
            }
        };
    let (network, route) = match (access, ctx.network) {
        (Access::SensitiveData, _) | (_, crate::egress::Network::Off) => {
            (duet_sandbox::Network::Off, None)
        }
        _ if offline => (duet_sandbox::Network::Off, None),
        (Access::Checks, _) if reads_protected => (duet_sandbox::Network::Off, None),
        (_, crate::egress::Network::All) => (duet_sandbox::Network::All, None),
        (_, crate::egress::Network::Registries(r)) => {
            match r.route(ctx.sandbox, ctx.audit, &guard).await {
                Ok(route) => (
                    duet_sandbox::Network::Proxy(route.network.clone()),
                    Some(route),
                ),
                Err(e) => {
                    note = Some(format!(
                        "\n[sandbox] the egress proxy could not start ({e}); the command ran without network\n"
                    ));
                    (duet_sandbox::Network::Off, None)
                }
            }
        }
    };
    // Package caches: filled by commands with network, read by the rest.
    let caches = match ctx.network {
        crate::egress::Network::Off => Vec::new(),
        _ => crate::egress::cache_env(
            &scratch_root().join(run_name(ctx.run_dir)),
            !network.is_off(),
        ),
    };
    // Placeholders in a sensitive_data command are resolved locally: its output
    // stays on this machine. Elsewhere they stay as written.
    let exec = match access {
        Access::SensitiveData => ctx.presenter.detokenize(command),
        _ => command.to_owned(),
    };
    let spec = Spec {
        workspace: ctx.workspace.to_path_buf(),
        scratch,
        network,
        timeout,
        output_cap: 256 * 1024,
        spill_file: Some(
            ctx.run_dir
                .join(format!("spill-{}.txt", uuid::Uuid::new_v4())),
        ),
        extra_env: {
            let mut env = vec![
                ("CARGO_TERM_COLOR".into(), "never".into()),
                ("NO_COLOR".into(), "1".into()),
                // Commands may not create `.git`: `cargo new` makes none.
                ("CARGO_CARGO_NEW_VCS".into(), "none".into()),
            ];
            env.extend(caches);
            // What a sensitive command builds may embed sensitive data (a
            // build script reading it): cargo builds into the command's private
            // scratch, not the shared `target/` other commands read and write.
            if access == Access::SensitiveData {
                env.push((
                    "CARGO_TARGET_DIR".into(),
                    sensitive_scratch(ctx.run_dir)
                        .join("cargo-target")
                        .display()
                        .to_string(),
                ));
            }
            env
        },
        deny_read,
        read_only: access == Access::ReadOnly,
    };
    let stop = async {
        match ctx.interrupted {
            Some(flag) => crate::run::raised(flag).await,
            None => std::future::pending().await,
        }
    };
    let mut o = duet_sandbox::run_until(
        ctx.sandbox,
        &spec,
        &["/bin/sh".into(), "-c".into(), exec],
        ctx.workspace,
        stop,
    )
    .await
    .map_err(|e| e.to_string())?;
    if o.interrupted {
        return Err("interrupted: the command was stopped".into());
    }
    // What the proxy refused, so the frontier knows why a download failed.
    if let Some(n) = note.or_else(|| {
        route
            .as_ref()
            .and_then(|r| crate::egress::refused_note(&r.refused()))
    }) {
        o.stderr.extend_from_slice(n.as_bytes());
    }
    drop(route);
    // Run state is denied to every command, so any command can meet a denial.
    if o.shows_denial() {
        ctx.record(AuditEvent::SandboxDenial {
            command: command.to_owned(),
            access: access_name.to_owned(),
        });
    }
    Ok(o)
}

pub(crate) fn render_output(o: &duet_sandbox::Output) -> Vec<u8> {
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
    let sensitive_data = args
        .get("sensitive_data")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let before = sensitive_data.then(|| snapshot(ctx.workspace));
    let started = std::time::SystemTime::now();
    let access = if sensitive_data {
        Access::SensitiveData
    } else {
        Access::Ordinary
    };
    let o = sandboxed(ctx, command, timeout, access).await?;
    let source = if let Some(before) = before {
        let after = snapshot(ctx.workspace);
        let mut changed: Vec<PathBuf> = after
            .iter()
            .filter(|(p, stamp)| before.get(*p) != Some(stamp))
            .map(|(p, _)| p.clone())
            .collect();
        changed.extend(build_output_changed(ctx.workspace, started));
        ctx.presenter.mark_sensitive(ctx.workspace, &changed);
        ctx.record(AuditEvent::SensitiveCommand {
            command: command.to_owned(),
            exit_code: o.exit_code,
            derived_files: changed.iter().map(|p| p.display().to_string()).collect(),
        });
        Source::SensitiveCommand {
            command: command.to_owned(),
            exit_code: o.exit_code,
        }
    } else {
        Source::Command {
            command: command.to_owned(),
            exit_code: o.exit_code,
        }
    };
    let mut shown = ctx.presenter.present(&source, &render_output(&o));
    // A command that tried to use `.git` is pointed at the git tools, or,
    // outside a repository, told that duet tracks the changes itself.
    if GIT_WORD.is_match(command)
        && (o.shows_denial() || String::from_utf8_lossy(&o.stderr).contains("not a git repository"))
    {
        shown.push('\n');
        shown.push_str(match ctx.git_tools {
            Some(_) => crate::git_tools::COMMAND_HINT,
            None => NO_REPOSITORY_HINT,
        });
    }
    Ok(shown)
}

/// The hint added to a `git` command's output outside a repository.
const NO_REPOSITORY_HINT: &str = "[this folder is not a git repository, and commands cannot create or \
use .git; you do not need one: list_files and search work without it, and diff shows the files you changed]";

static GIT_WORD: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"(^|[\s;&|(`])git(\s|$)").expect("static regex"));

/// Directories whose files are build output or dependencies, not data.
const SNAPSHOT_SKIP: &[&str] = &[".git", ".duet", "target", "node_modules"];
/// Directories [`snapshot`] skips that a command may still write data into.
const BUILD_OUTPUT: &[&str] = &["target", "node_modules"];
/// How far before a command's start a file's modification time may lie and
/// still count as written by it (file systems stamp times coarsely).
const MTIME_SLACK: Duration = Duration::from_secs(1);

/// Files under build output and dependency directories (`target/`,
/// `node_modules/`, at any depth) modified since `since`: what a command
/// wrote there. [`snapshot`] skips these directories (they are large and
/// hold no data of their own), so a sensitive command's writes into them are
/// found by modification time alone: one pass over them, no copy of their
/// state before the command.
fn build_output_changed(workspace: &Path, since: std::time::SystemTime) -> Vec<PathBuf> {
    let since = since.checked_sub(MTIME_SLACK).unwrap_or(since);
    let mut out = Vec::new();
    // (directory, inside build output)
    let mut stack = vec![(PathBuf::new(), false)];
    while let Some((rel, inside)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(workspace.join(&rel)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let child = rel.join(name.as_ref());
            match entry.metadata() {
                Ok(m) if m.is_dir() => {
                    if name == ".git" || name == ".duet" {
                        continue;
                    }
                    stack.push((child, inside || BUILD_OUTPUT.contains(&name.as_ref())));
                }
                Ok(m) if m.is_file() && inside && m.modified().is_ok_and(|t| t >= since) => {
                    out.push(child);
                }
                _ => {}
            }
        }
    }
    out.sort();
    out
}

/// Modification time and size of every workspace file outside build output.
fn snapshot(workspace: &Path) -> std::collections::BTreeMap<PathBuf, (std::time::SystemTime, u64)> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(workspace.join(&rel)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let child = rel.join(&name);
            match entry.metadata() {
                Ok(m) if m.is_dir() => {
                    if !SNAPSHOT_SKIP.contains(&name.to_string_lossy().as_ref()) {
                        stack.push(child);
                    }
                }
                Ok(m) if m.is_file() => {
                    let modified = m.modified().unwrap_or(std::time::UNIX_EPOCH);
                    out.insert(child, (modified, m.len()));
                }
                _ => {}
            }
        }
    }
    out
}

async fn finish(ctx: &mut Ctx<'_>, args: &Map<String, Value>) -> Outcome {
    let summary = args
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let (failed, report) = run_checks(ctx, ctx.checks).await;
    if failed {
        Outcome::ChecksFailed(ctx.presenter.present(&Source::Checks, &report))
    } else {
        Outcome::Finished { summary }
    }
}

/// Runs `commands` as checks; returns whether any failed, and the report.
pub(crate) async fn run_checks(ctx: &Ctx<'_>, commands: &[String]) -> (bool, Vec<u8>) {
    let mut report = Vec::new();
    let mut failed = false;
    for check in commands {
        match sandboxed(ctx, check, ctx.command_timeout, Access::Checks).await {
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
    (failed, report)
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
    fn every_other_process_denies_the_sensitive_commands_scratch() {
        // Servers start before the first sensitive command: the directory must
        // already exist, so the sandbox covers it as a directory.
        let d = tempfile::tempdir().unwrap();
        let run = d.path().join(format!("t{}", uuid::Uuid::new_v4().simple()));
        let passthrough = duet_boundary::view::PassThrough { max_bytes: 100 };
        let hidden = hidden_from_processes(&passthrough, d.path(), &run);
        let scratch = sensitive_scratch(&run);
        assert_eq!(hidden, vec![scratch.clone()]);
        assert!(scratch.is_dir());
        assert_ne!(scratch, scratch_root().join(run_name(&run)));
        std::fs::remove_dir(scratch).unwrap();
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

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn finish_requires_the_acceptance_command_to_pass_after_a_repair() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().canonicalize().unwrap();
        let run = ws.join(".duet/runs/check");
        std::fs::create_dir_all(&run).unwrap();
        let git = Git::locate().unwrap();
        let presenter = duet_boundary::view::PassThrough { max_bytes: 4096 };
        let mut journal = WriteJournal::open(&run).unwrap();
        let checks = vec!["test -f accepted.txt".to_owned()];
        let mut ctx = Ctx {
            workspace: &ws,
            run_dir: &run,
            sandbox: duet_sandbox::detect().unwrap(),
            git: &git,
            presenter: &presenter,
            journal: &mut journal,
            command_timeout: Duration::from_secs(5),
            network: &crate::egress::Network::Off,
            checks: &checks,
            audit: None,
            interrupted: None,
            web: None,
            git_tools: None,
            lsp: None,
        };
        let args = json!({"summary": "ready"});
        let Value::Object(args) = args else {
            unreachable!()
        };
        assert!(matches!(
            dispatch(&mut ctx, "finish", &args).await,
            Outcome::ChecksFailed(_)
        ));
        std::fs::write(ws.join("accepted.txt"), "fixed\n").unwrap();
        assert_eq!(
            dispatch(&mut ctx, "finish", &args).await,
            Outcome::Finished {
                summary: "ready".into()
            }
        );
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod sensitive_command_tests {
    use super::*;
    use duet_boundary::engine::Engine;
    use duet_boundary::policy::Policy;

    const BALANCE: &str = "8977066";

    async fn call(ctx: &mut Ctx<'_>, name: &str, args: Value) -> String {
        let Value::Object(args) = args else {
            unreachable!()
        };
        match dispatch(ctx, name, &args).await {
            Outcome::Result(s) | Outcome::Error(s) => s,
            other => format!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn commands_cannot_read_git_history_or_run_state() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        let run = ws.join(".duet/runs/r1");
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::create_dir_all(&run).unwrap();
        std::fs::create_dir_all(ws.join(".duet/tmp")).unwrap();
        std::fs::write(
            ws.join("data/orders.csv"),
            format!("id,total\n1,{BALANCE}\n"),
        )
        .unwrap();
        std::fs::write(ws.join("README.md"), "public\n").unwrap();
        // The data is committed, as the eval workspaces commit their starter.
        let git_bin = Git::locate().unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["add", "-A"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t.test",
                "commit",
                "-qm",
                "start",
            ],
        ] {
            let ok = std::process::Command::new("/usr/bin/git")
                .args(&args)
                .current_dir(&ws)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        }
        let policy = Policy {
            sensitive_globs: vec!["data/**".into()],
            command_output_sensitive: true,
            detect_pii: true,
            ..Policy::default()
        };
        let engine = Engine::open(&run, policy, None).unwrap();
        // The vault holds the real value once the engine has seen the file.
        engine.prime(&ws, &["data/orders.csv".to_string()], "");
        assert!(
            std::fs::read_to_string(run.join("vault.json"))
                .unwrap()
                .contains(BALANCE)
        );
        let mut journal = WriteJournal::open(&run).unwrap();
        let mut ctx = Ctx {
            workspace: &ws,
            run_dir: &run,
            sandbox: duet_sandbox::detect().unwrap(),
            git: &git_bin,
            presenter: engine.as_ref(),
            journal: &mut journal,
            command_timeout: Duration::from_secs(30),
            network: &crate::egress::Network::Off,
            checks: &[],
            audit: None,
            interrupted: None,
            web: None,
            git_tools: None,
            lsp: None,
        };
        let out = call(
            &mut ctx,
            "run_command",
            json!({"command": "git show HEAD:data/orders.csv | od -c; \
                cat .git/objects/*/* | od -c | head; \
                od -c .duet/runs/r1/vault.json; \
                echo scratch-ok > \"$TMPDIR/t\" && cat \"$TMPDIR/t\"; cat README.md"}),
        )
        .await;
        assert!(!out.contains("  8   9   7"), "{out}");
        assert!(
            out.contains("scratch-ok") && out.contains("public"),
            "{out}"
        );
    }

    #[tokio::test]
    async fn read_file_never_reads_run_state_or_git_internals() {
        // An ordinary command's `$TMPDIR` names the run, so the frontier can
        // know where its run state is; the raw handles there hold what no
        // detector recognizes (a date of birth).
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        let run = ws.join(".duet/runs/r1");
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::create_dir_all(ws.join(".git")).unwrap();
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(ws.join(".git/config"), "[core]\n").unwrap();
        let row = "id,born,total\n1,1987-03-14,8977066\n";
        std::fs::write(ws.join("data/orders.csv"), row).unwrap();
        let policy = Policy {
            sensitive_globs: vec!["data/**".into()],
            command_output_sensitive: true,
            detect_pii: true,
            ..Policy::default()
        };
        let engine = Engine::open(&run, policy, None).unwrap();
        let git = Git::locate().unwrap();
        let mut journal = WriteJournal::open(&run).unwrap();
        let mut ctx = Ctx {
            workspace: &ws,
            run_dir: &run,
            sandbox: duet_sandbox::detect().unwrap(),
            git: &git,
            presenter: engine.as_ref(),
            journal: &mut journal,
            command_timeout: Duration::from_secs(30),
            network: &crate::egress::Network::Off,
            checks: &[],
            audit: None,
            interrupted: None,
            web: None,
            git_tools: None,
            lsp: None,
        };
        let shown = call(&mut ctx, "read_file", json!({"path": "data/orders.csv"})).await;
        assert!(
            shown.contains("h1") && !shown.contains("1987-03-14"),
            "{shown}"
        );
        assert!(run.join("handles/h1").is_file());
        for path in [
            ".duet/runs/r1/handles/h1",
            "./.duet/runs/r1/vault.json",
            ".git/config",
        ] {
            let out = call(&mut ctx, "read_file", json!({"path": path})).await;
            assert!(out.contains("is not available"), "{path}: {out}");
            assert!(
                !out.contains("1987-03-14") && !out.contains("[core]"),
                "{out}"
            );
        }
    }

    #[tokio::test]
    async fn commands_cannot_read_sensitive_files_unless_their_output_stays_local() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        let run = d.path().canonicalize().unwrap().join("run");
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::create_dir_all(ws.join(".duet")).unwrap();
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            ws.join("data/orders.csv"),
            format!("id,total\n1,{BALANCE}\n"),
        )
        .unwrap();
        std::fs::write(ws.join("app.log"), "INFO ok\n").unwrap();
        let policy = Policy {
            sensitive_globs: vec!["data/**".into(), "*.log".into()],
            command_output_sensitive: true,
            detect_pii: true,
            ..Policy::default()
        };
        let engine = Engine::open(&run, policy, None).unwrap();
        let git = Git::locate().unwrap();
        let mut journal = WriteJournal::open(&run).unwrap();
        let audit_path = run.join("audit.jsonl");
        let audit = AuditHandle::new(duet_boundary::audit::AuditLog::open(&audit_path).unwrap());
        let mut ctx = Ctx {
            workspace: &ws,
            run_dir: &run,
            sandbox: duet_sandbox::detect().unwrap(),
            git: &git,
            presenter: engine.as_ref(),
            journal: &mut journal,
            command_timeout: Duration::from_secs(30),
            network: &crate::egress::Network::Off,
            checks: &[],
            audit: Some(&audit),
            interrupted: None,
            web: None,
            git_tools: None,
            lsp: None,
        };

        // Any encoding of the data is out of reach of an ordinary command.
        let plain = call(
            &mut ctx,
            "run_command",
            json!({"command": "od -c data/orders.csv; cat app.log; ls data"}),
        )
        .await;
        assert!(plain.contains(duet_sandbox::DENIAL_MESSAGE), "{plain}");
        assert!(!plain.contains("  8   9   7"), "{plain}");
        assert!(!plain.contains("orders.csv\n"), "listing hidden: {plain}");

        // With sensitive_data the command reads it, but the output is held locally
        // and what the command wrote becomes sensitive.
        let held = call(
            &mut ctx,
            "run_command",
            json!({"command": "cut -d, -f2 data/orders.csv > totals.txt; cat totals.txt", "sensitive_data": true}),
        )
        .await;
        assert!(!held.contains(BALANCE), "{held}");
        assert!(held.contains("ask_local"), "{held}");
        let derived = call(&mut ctx, "read_file", json!({"path": "totals.txt"})).await;
        assert!(!derived.contains(BALANCE), "{derived}");
        let after = call(
            &mut ctx,
            "run_command",
            json!({"command": "cat totals.txt"}),
        )
        .await;
        assert!(
            !after.contains(BALANCE) && after.contains(duet_sandbox::DENIAL_MESSAGE),
            "{after}"
        );

        // The audit log names the decisions, never the data.
        let log = std::fs::read_to_string(&audit_path).unwrap();
        assert!(!log.contains(BALANCE), "{log}");
        let kinds: Vec<String> = duet_boundary::audit::read(&audit_path)
            .unwrap()
            .into_iter()
            .filter_map(|l| match l {
                duet_boundary::audit::Line::Event(e) => Some(e.event.kind().to_owned()),
                _ => None,
            })
            .collect();
        assert_eq!(
            kinds,
            ["sandbox_denial", "sensitive_command", "sandbox_denial"]
        );
        assert!(log.contains("\"derived_files\":[\"totals.txt\"]"), "{log}");
    }

    #[tokio::test]
    async fn what_a_sensitive_command_writes_into_build_output_stays_sensitive() {
        // Found by the privacy scenarios: the snapshot skips `target/` and
        // `node_modules/`, so a transformed copy written there was public.
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        let run = d
            .path()
            .canonicalize()
            .unwrap()
            .join(format!("t{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::create_dir_all(ws.join("target/debug")).unwrap();
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            ws.join("data/orders.csv"),
            format!("id,total\n1,{BALANCE}\n"),
        )
        .unwrap();
        // Build output from before the command is not its doing.
        let old = ws.join("target/debug/old.txt");
        std::fs::write(&old, "built earlier\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
        let policy = Policy {
            sensitive_globs: vec!["data/**".into()],
            command_output_sensitive: true,
            detect_pii: true,
            ..Policy::default()
        };
        let engine = Engine::open(&run, policy, None).unwrap();
        let git = Git::locate().unwrap();
        let mut journal = WriteJournal::open(&run).unwrap();
        let audit_path = run.join("audit.jsonl");
        let audit = AuditHandle::new(duet_boundary::audit::AuditLog::open(&audit_path).unwrap());
        let mut ctx = Ctx {
            workspace: &ws,
            run_dir: &run,
            sandbox: duet_sandbox::detect().unwrap(),
            git: &git,
            presenter: engine.as_ref(),
            journal: &mut journal,
            command_timeout: Duration::from_secs(30),
            network: &crate::egress::Network::Off,
            checks: &[],
            audit: Some(&audit),
            interrupted: None,
            web: None,
            git_tools: None,
            lsp: None,
        };
        let held = call(
            &mut ctx,
            "run_command",
            json!({"command": "rev data/orders.csv > target/export.txt; \
                mkdir -p web/node_modules/.cache && cp data/orders.csv web/node_modules/.cache/rows; \
                echo \"cargo builds in $CARGO_TARGET_DIR\"", "sensitive_data": true}),
        )
        .await;
        // Cargo builds privately: in the command's own scratch directory.
        let handle = held.split_whitespace().next().unwrap();
        let raw = std::fs::read_to_string(run.join("handles").join(handle)).unwrap();
        let private = sensitive_scratch(&run).join("cargo-target");
        assert!(
            raw.contains(&format!("cargo builds in {}", private.display())),
            "{raw}"
        );
        let log = std::fs::read_to_string(&audit_path).unwrap();
        assert!(
            log.contains(
                "\"derived_files\":[\"target/export.txt\",\"web/node_modules/.cache/rows\"]"
            ),
            "{log}"
        );
        let reversed: String = BALANCE.chars().rev().collect();
        for path in ["target/export.txt", "web/node_modules/.cache/rows"] {
            let read = call(&mut ctx, "read_file", json!({"path": path})).await;
            assert!(
                !read.contains(&reversed) && !read.contains(BALANCE),
                "{read}"
            );
            assert!(read.contains("ask_local"), "{read}");
            let cat = call(
                &mut ctx,
                "run_command",
                json!({"command": format!("cat {path}")}),
            )
            .await;
            assert!(cat.contains(duet_sandbox::DENIAL_MESSAGE), "{cat}");
        }
        let earlier = call(
            &mut ctx,
            "run_command",
            json!({"command": "cat target/debug/old.txt"}),
        )
        .await;
        assert!(earlier.contains("built earlier"), "{earlier}");
        let _ = std::fs::remove_dir_all(sensitive_scratch(&run));
    }

    #[tokio::test]
    async fn sensitive_commands_never_reach_run_state_and_resolve_placeholders_locally() {
        // Seen in a live run: a `sensitive_data` command (no deny list at all)
        // listed `.duet`, read the vault and the transcripts, and found the
        // value the operator had typed.
        const CARD: &str = "4539578763621486";
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        let id = format!("t{}", uuid::Uuid::new_v4().simple());
        let run = ws.join(".duet/runs").join(&id);
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::create_dir_all(&run).unwrap();
        std::fs::create_dir_all(ws.join(".duet/audit")).unwrap();
        std::fs::write(ws.join("data/cards.csv"), format!("id,card\n1,{CARD}\n")).unwrap();
        let policy = Policy {
            sensitive_globs: vec!["data/**".into()],
            command_output_sensitive: true,
            detect_pii: true,
            ..Policy::default()
        };
        let engine = Engine::open(&run, policy, None).unwrap();
        engine.prime(&ws, &["data/cards.csv".to_string()], "");
        let typed = engine.sanitize_message(CARD);
        assert!(!typed.contains(CARD), "{typed}");
        let token = duet_boundary::vault::Vault::tokens_in(&typed)[0].clone();
        let audit_path = ws.join(".duet/audit").join(format!("{id}.jsonl"));
        let audit = AuditHandle::new(duet_boundary::audit::AuditLog::open(&audit_path).unwrap());
        audit.record(AuditEvent::RunStart {
            mode: "hybrid".into(),
            boundary: true,
        });
        let git = Git::locate().unwrap();
        let mut journal = WriteJournal::open(&run).unwrap();
        let mut ctx = Ctx {
            workspace: &ws,
            run_dir: &run,
            sandbox: duet_sandbox::detect().unwrap(),
            git: &git,
            presenter: engine.as_ref(),
            journal: &mut journal,
            command_timeout: Duration::from_secs(30),
            network: &crate::egress::Network::Off,
            checks: &[],
            audit: Some(&audit),
            interrupted: None,
            web: None,
            git_tools: None,
            lsp: None,
        };
        let scratch = std::env::temp_dir()
            .join("duet-scratch")
            .join(format!("{id}-sensitive"));
        let shown = call(
            &mut ctx,
            "run_command",
            json!({"command": format!(
                "od -c .duet/runs/{id}/vault.json; ls -R .duet; grep -rl 4539 .duet; \
                 echo forged >> .duet/audit/{id}.jsonl; cat data/cards.csv; \
                 echo 'resolved {token}' > \"$TMPDIR/c\"; cat \"$TMPDIR/c\""
            ), "sensitive_data": true}),
        )
        .await;
        assert!(!shown.contains(CARD), "{shown}");
        // The raw output, held locally: the data file was readable, run state was not.
        let handle = shown.split_whitespace().next().unwrap();
        let held = std::fs::read_to_string(run.join("handles").join(handle)).unwrap();
        assert!(held.contains(&format!("1,{CARD}")), "{held}");
        assert!(held.contains(&format!("resolved {CARD}")), "{held}");
        assert!(
            !held.contains("   4   5   3   9") && !held.contains("vault.json\n"),
            "{held}"
        );
        assert!(held.contains(duet_sandbox::DENIAL_MESSAGE), "{held}");
        assert!(
            std::fs::read_to_string(scratch.join("c"))
                .unwrap()
                .contains(CARD)
        );

        // An ordinary command keeps placeholders as written and cannot read
        // what a sensitive command left in its temporary directory.
        call(
            &mut ctx,
            "run_command",
            json!({"command": format!(
                "echo '{token}' > literal.txt; cat {}/c > leak.txt", scratch.display()
            )}),
        )
        .await;
        assert_eq!(
            std::fs::read_to_string(ws.join("literal.txt")).unwrap(),
            format!("{token}\n")
        );
        assert!(
            !std::fs::read_to_string(ws.join("leak.txt"))
                .unwrap()
                .contains(CARD)
        );

        // The audit log was not written by the command, names no value, and verifies.
        let log = std::fs::read_to_string(&audit_path).unwrap();
        assert!(
            !log.lines().any(|l| l == "forged") && !log.contains(CARD),
            "{log}"
        );
        assert!(log.contains("sandbox_denial"), "{log}");
        assert!(matches!(
            duet_boundary::audit::verify(&audit_path).unwrap(),
            duet_boundary::audit::Verification::Intact { .. }
        ));
        let _ = std::fs::remove_dir_all(scratch);
    }
}
