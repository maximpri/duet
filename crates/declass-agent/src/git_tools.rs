// SPDX-License-Identifier: GPL-3.0-or-later
//! Git tools: `git_status`, `git_log`, `git_show`, `git_blame` and
//! `git_commit`, offered when the workspace is a git repository.
//!
//! Every git call goes through the hardened `declass-git` runner (no hooks, no
//! global or system configuration, no external diff or filter programs);
//! commands still cannot read `.git`.
//!
//! History is presented by path, as [`Source::GitHistory`]: each file of a
//! commit is presented on its own, so a path that is sensitive now stays
//! sensitive in every revision, a protected path's history is withheld, and a
//! public path's old diffs are scanned for secrets. Hidden and sealed paths
//! never appear. Commit metadata (messages, authors) is scanned too.
//!
//! `git_commit` records only files this run wrote (its write journal), never
//! sensitive, derived or protected ones, with a message free of placeholders
//! and known sensitive values, as the operator (`git.author`, else `user.name`
//! and `user.email` from the repository's or the owner's git configuration).
//! It never pushes, resets, checks out or switches branches, and repository
//! hooks never run (so there is no `git.run_hooks`: a hook is code the
//! repository chooses, and it would run unsandboxed with the operator's
//! rights). `git.commit` decides whether it is offered and whether the
//! operator approves each commit (see [`CommitPolicy`]).

use crate::oversight::Oversight;
use crate::tools::{Ctx, fs_err, string_arg};
use declass_boundary::audit::AuditEvent;
use declass_boundary::model::ToolSpec;
use declass_boundary::policy::IpLevel;
use declass_boundary::view::Source;
use declass_git::{Git, Identity};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// The tools this module handles.
pub const NAMES: [&str; 5] = [
    "git_blame",
    "git_commit",
    "git_log",
    "git_show",
    "git_status",
];

const DEFAULT_LOG: u64 = 20;
const MAX_LOG: u64 = 100;
/// Files of one commit whose diffs `git_show` presents.
const MAX_SHOW_FILES: usize = 30;
/// Presented diff text of one `git_show` before further files are only listed.
const MAX_SHOW_CHARS: usize = 200_000;
const MAX_BLAME_LINES: usize = 400;
const MAX_MESSAGE_CHARS: usize = 8_000;
/// The run's base commit, recorded before its first commit, so `diff` keeps
/// showing everything the run changed.
const BASE_FILE: &str = "git-base";

/// `git.commit`: whether `git_commit` is offered and asks the operator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CommitPolicy {
    /// Offered; commits without asking (still audited, still only the run's files).
    Allow,
    /// Offered only when the operator can be asked (the run has an approver:
    /// `oversight.approve` is not `off`, or an interactive `declass` session), and
    /// every commit waits for their approval. The default: a commit is
    /// permanent history in the operator's repository.
    #[default]
    Ask,
    /// Never offered.
    Off,
}

impl std::str::FromStr for CommitPolicy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "allow" => Ok(Self::Allow),
            "ask" => Ok(Self::Ask),
            "off" => Ok(Self::Off),
            other => Err(format!("git.commit must be allow, ask or off, not {other}")),
        }
    }
}

/// The git tools of a run, decided once when it starts (the tool set is fixed).
#[derive(Debug, Clone)]
pub struct GitTools {
    /// Whether `git_commit` is offered.
    pub commit: bool,
    /// The operator's identity for commits (`git.author`), if set.
    pub author: Option<Identity>,
    /// The owner's git configuration files, read for `user.name` and
    /// `user.email` only when `author` is not set.
    pub user_files: Vec<PathBuf>,
}

impl GitTools {
    /// The git tools of a run or session configured by `cfg`; `None` when
    /// the workspace is not a git repository (no git tools).
    pub fn for_run(git: &Git, cfg: &crate::run::RunConfig) -> Option<Self> {
        Self::decide(git, &cfg.workspace, &cfg.oversight, cfg.git_author.clone())
    }

    /// [`GitTools::for_run`] from its parts.
    pub fn decide(
        git: &Git,
        workspace: &Path,
        oversight: &Oversight,
        author: Option<Identity>,
    ) -> Option<Self> {
        if !git.is_repository(workspace) {
            return None;
        }
        let commit = match oversight.git_commit {
            CommitPolicy::Allow => true,
            CommitPolicy::Ask => oversight.approver.is_some(),
            CommitPolicy::Off => false,
        };
        Some(Self {
            commit,
            author,
            user_files: declass_git::user_config_files(),
        })
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        let t = |name: &str, description: &str, parameters: Value| ToolSpec {
            name: name.to_owned(),
            description: description.to_owned(),
            parameters,
        };
        let mut specs = vec![
            t(
                "git_status",
                "Show the repository's branch and changed, staged and untracked files.",
                json!({"type": "object", "properties": {}}),
            ),
            t(
                "git_log",
                "List commits (hash, date, author, subject), newest first. Optionally only those touching `path`, \
or reachable from `rev`.",
                json!({"type": "object", "properties": {
                    "path": {"type": "string", "description": "Only commits that changed this path."},
                    "rev": {"type": "string", "description": "Start from this revision or range (default HEAD)."},
                    "max_count": {"type": "integer", "minimum": 1, "maximum": MAX_LOG}
                }}),
            ),
            t(
                "git_show",
                "Show a commit (message, changed files and their diffs), or with `path` that file's content at the \
revision.",
                json!({"type": "object", "properties": {
                    "rev": {"type": "string", "description": "A commit: hash, branch, tag or HEAD~N."},
                    "path": {"type": "string", "description": "Show this file as it was at `rev`."}
                }, "required": ["rev"]}),
            ),
            t(
                "git_blame",
                "Show who last changed each line of a file, and in which commit (at most 400 lines per call).",
                json!({"type": "object", "properties": {
                    "path": {"type": "string"},
                    "start_line": {"type": "integer", "minimum": 1},
                    "end_line": {"type": "integer", "minimum": 1}
                }, "required": ["path"]}),
            ),
        ];
        if self.commit {
            specs.push(t(
                "git_commit",
                "Commit files you wrote in this run to the current branch, as the operator. Default: every file you \
wrote that may be committed; `paths` commits a subset. Sensitive and protected files are never committed, and the \
message must not contain placeholders. Nothing is pushed.",
                json!({"type": "object", "properties": {
                    "message": {"type": "string"},
                    "paths": {"type": "array", "items": {"type": "string"},
                        "description": "Files to commit (each one you wrote in this run)."}
                }, "required": ["message"]}),
            ));
        }
        specs
    }
}

pub(crate) fn dispatch(
    ctx: &mut Ctx<'_>,
    name: &str,
    args: &Map<String, Value>,
) -> Result<String, String> {
    let Some(tools) = ctx.git_tools else {
        return Err("git tools are not available: the workspace is not a git repository".into());
    };
    match name {
        "git_status" => status(ctx),
        "git_log" => log(ctx, args),
        "git_show" => show(ctx, args),
        "git_blame" => blame(ctx, args),
        "git_commit" if tools.commit => commit(ctx, tools, args),
        "git_commit" => Err("git_commit is not available in this run (git.commit)".into()),
        other => Err(format!("unknown tool `{other}`")),
    }
}

/// The hint added to a command's output when it tried to use `.git`.
pub(crate) const COMMAND_HINT: &str = "[.git is not available to commands: use the git_status, git_log, git_show, \
git_blame and git_commit tools]";

fn git_err(e: declass_git::GitError) -> String {
    e.to_string()
}

/// Whether history may name `path` at all: hidden, reserved and sealed paths never appear.
fn hidden(ctx: &Ctx<'_>, path: &Path) -> bool {
    declass_fs::is_reserved(path)
        || !ctx.presenter.path_visible(path)
        || ctx.presenter.protection(path) == Some(IpLevel::Sealed)
}

/// A model-supplied path that history may show.
fn history_path(ctx: &Ctx<'_>, raw: &str) -> Result<PathBuf, String> {
    let rel = declass_fs::normalize_relative(raw).map_err(fs_err)?;
    if hidden(ctx, &rel) {
        return Err(format!("{raw} is not available"));
    }
    Ok(rel)
}

/// A model-supplied revision: never an option, no whitespace or control characters.
fn check_rev(rev: &str) -> Result<&str, String> {
    let rev = rev.trim();
    if rev.is_empty()
        || rev.starts_with('-')
        || rev.len() > 200
        || rev.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(format!("invalid revision `{rev}`"));
    }
    Ok(rev)
}

fn run_text(ctx: &Ctx<'_>, args: &[&str]) -> Result<String, String> {
    ctx.git
        .run(ctx.workspace, args, &[], None)
        .map(|o| String::from_utf8_lossy(&o).into_owned())
        .map_err(git_err)
}

/// The full hash of the commit `rev` names.
fn resolve(ctx: &Ctx<'_>, rev: &str) -> Result<String, String> {
    let spec = format!("{rev}^{{commit}}");
    run_text(
        ctx,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ],
    )
    .map(|s| s.trim().to_owned())
    .map_err(|_| format!("unknown revision `{rev}`"))
}

fn history(ctx: &Ctx<'_>, rev: &str, path: Option<&Path>, text: &str) -> String {
    ctx.presenter.present(
        &Source::GitHistory {
            rev: rev.to_owned(),
            path: path.map(Path::to_path_buf),
        },
        text.as_bytes(),
    )
}

fn status(ctx: &Ctx<'_>) -> Result<String, String> {
    let prefix = ctx.git.prefix(ctx.workspace).map_err(git_err)?;
    let raw = ctx
        .git
        .run(
            ctx.workspace,
            &[
                "status",
                "--porcelain=v1",
                "-b",
                "-z",
                "--untracked-files=all",
                "--no-renames",
            ],
            &[],
            None,
        )
        .map_err(git_err)?;
    let mut out = String::new();
    let mut changes = 0;
    for rec in raw.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        let rec = String::from_utf8_lossy(rec);
        if let Some(branch) = rec.strip_prefix("## ") {
            out.push_str(&format!("branch: {branch}\n"));
            continue;
        }
        let (code, path) = rec.split_at(rec.len().min(3));
        // Paths are relative to the repository root; only the workspace's own count.
        let Some(rel) = path.strip_prefix(prefix.as_str()) else {
            continue;
        };
        let rel_path = Path::new(rel);
        if declass_fs::is_reserved(rel_path) || !ctx.presenter.path_visible(rel_path) {
            continue;
        }
        let mark = match ctx.presenter.protection(rel_path) {
            Some(IpLevel::Sealed) => "  [sealed]",
            Some(IpLevel::InterfaceOnly) => "  [interface-only]",
            None => "",
        };
        changes += 1;
        out.push_str(&format!("{code}{rel}{mark}\n"));
    }
    if changes == 0 {
        out.push_str("no changes\n");
    }
    Ok(history(ctx, "working tree", None, &out))
}

fn log(ctx: &Ctx<'_>, args: &Map<String, Value>) -> Result<String, String> {
    let max = args
        .get("max_count")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_LOG)
        .clamp(1, MAX_LOG);
    let rev = match args.get("rev").and_then(Value::as_str) {
        Some(r) => check_rev(r)?.to_owned(),
        None => {
            if ctx.git.head(ctx.workspace).map_err(git_err)?.is_none() {
                return Ok("no commits yet".into());
            }
            "HEAD".to_owned()
        }
    };
    let path = match args.get("path").and_then(Value::as_str) {
        Some(p) => Some(history_path(ctx, p)?),
        None => None,
    };
    let count = format!("--max-count={max}");
    let mut cmd = vec![
        "log",
        "--no-color",
        "--no-show-signature",
        "--date=format:%Y-%m-%d %H:%M",
        "--format=%h%x1f%ad%x1f%an <%ae>%x1f%s%x1e",
        &count,
        "--end-of-options",
        &rev,
        "--",
    ];
    let p = path.as_ref().map(|p| p.to_string_lossy().into_owned());
    cmd.extend(p.as_deref());
    let raw = run_text(ctx, &cmd).map_err(|e| format!("git log failed: {e}"))?;
    let mut out = String::new();
    let mut n = 0;
    for rec in raw.split('\x1e').map(str::trim).filter(|r| !r.is_empty()) {
        let fields: Vec<&str> = rec.split('\x1f').collect();
        if let [hash, date, author, subject] = fields[..] {
            n += 1;
            out.push_str(&format!("{hash}  {date}  {author}  {subject}\n"));
        }
    }
    if n == 0 {
        return Ok("no commits".into());
    }
    if n as u64 == max {
        out.push_str(&format!(
            "[the latest {max} commits; raise max_count (up to {MAX_LOG}) or start from an older rev for more]\n"
        ));
    }
    Ok(history(ctx, &rev, None, &out))
}

fn numbered(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let width = lines.len().max(1).to_string().len();
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{:>width$}  {l}\n", i + 1))
        .collect()
}

fn show(ctx: &Ctx<'_>, args: &Map<String, Value>) -> Result<String, String> {
    let rev = check_rev(string_arg(args, "rev")?)?;
    let hash = resolve(ctx, rev)?;
    let short: String = hash.chars().take(12).collect();
    if let Some(raw) = args.get("path").and_then(Value::as_str) {
        let path = history_path(ctx, raw)?;
        let object = format!("{hash}:./{}", path.display());
        let size: u64 = run_text(ctx, &["cat-file", "-s", &object])
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| format!("{raw} does not exist at {rev}"))?;
        if size > crate::tools::MAX_READ_BYTES {
            return Err(format!(
                "{raw} at {rev} is too large to show ({size} bytes)"
            ));
        }
        let bytes = ctx
            .git
            .run(ctx.workspace, &["cat-file", "blob", &object], &[], None)
            .map_err(|_| format!("{raw} is not a file at {rev}"))?;
        if bytes.contains(&0) {
            return Ok(format!(
                "{} at {short}: binary file, {size} bytes",
                path.display()
            ));
        }
        let text = String::from_utf8_lossy(&bytes);
        let header = format!(
            "{} at {short} ({} lines)\n",
            path.display(),
            text.lines().count()
        );
        return Ok(header + &history(ctx, &short, Some(&path), &numbered(&text)));
    }
    let meta = run_text(
        ctx,
        &[
            "show",
            "-s",
            "--no-color",
            "--no-show-signature",
            "--date=iso-strict",
            "--format=commit %H%nAuthor: %an <%ae>%nDate:   %ad%n%n%B",
            &hash,
        ],
    )?;
    let mut out = history(ctx, &short, None, &meta);
    let diff_args = [
        "show",
        "--no-color",
        "--format=",
        "--no-renames",
        "--relative",
        "--diff-merges=first-parent",
        "--no-ext-diff",
        "--no-textconv",
    ];
    let mut names = diff_args.to_vec();
    names.extend(["--name-status", "-z", &hash]);
    let listed = ctx
        .git
        .run(ctx.workspace, &names, &[], None)
        .map_err(git_err)?;
    let fields: Vec<String> = listed
        .split(|&b| b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect();
    let files: Vec<(&str, &str)> = fields
        .chunks(2)
        .filter_map(|c| match c {
            [status, path] => Some((status.as_str(), path.as_str())),
            _ => None,
        })
        .filter(|(_, p)| !hidden(ctx, Path::new(p)))
        .collect();
    if files.is_empty() {
        out.push_str("\n(no file changes to show)\n");
        return Ok(out);
    }
    out.push_str(&format!("\nFiles changed ({}):\n", files.len()));
    for (status, path) in &files {
        out.push_str(&format!("{status}  {path}\n"));
    }
    let mut shown = 0;
    let mut size = 0;
    for (_, path) in &files {
        if shown == MAX_SHOW_FILES || size > MAX_SHOW_CHARS {
            break;
        }
        let mut patch = diff_args.to_vec();
        patch.extend(["--patch", &hash, "--", path]);
        let diff = run_text(ctx, &patch)?;
        if diff.trim().is_empty() {
            continue;
        }
        let view = history(ctx, &short, Some(Path::new(path)), &diff);
        size += view.len();
        shown += 1;
        out.push('\n');
        out.push_str(&view);
        if !view.ends_with('\n') {
            out.push('\n');
        }
    }
    if shown < files.len() {
        out.push_str(&format!(
            "\n[diffs of {} more file(s) not shown; git_show with `path` shows a file at this commit]\n",
            files.len() - shown
        ));
    }
    Ok(out)
}

fn blame(ctx: &Ctx<'_>, args: &Map<String, Value>) -> Result<String, String> {
    let raw = string_arg(args, "path")?;
    let path = history_path(ctx, raw)?;
    let bytes = declass_fs::read_file(ctx.workspace, &path, crate::tools::MAX_READ_BYTES)
        .map_err(fs_err)?;
    let total = String::from_utf8_lossy(&bytes).lines().count();
    if total == 0 {
        return Ok(format!("{raw} is empty"));
    }
    let start = args
        .get("start_line")
        .and_then(Value::as_u64)
        .map_or(1, |s| s.max(1) as usize);
    if start > total {
        return Err(format!("{raw} has {total} lines"));
    }
    let asked_end = args
        .get("end_line")
        .and_then(Value::as_u64)
        .map_or(total, |e| e as usize);
    if asked_end < start {
        return Err(format!("end_line {asked_end} is before start_line {start}"));
    }
    let end = asked_end.min(total).min(start + MAX_BLAME_LINES - 1);
    let range = format!("{start},{end}");
    let p = path.to_string_lossy().into_owned();
    let porcelain = run_text(
        ctx,
        &[
            "blame",
            "--line-porcelain",
            "--no-textconv",
            "-L",
            &range,
            "--",
            &p,
        ],
    )
    .map_err(|e| format!("git blame failed: {e}"))?;
    // Consecutive lines from one commit are grouped under a `commit` line;
    // each line keeps its number, as `read_file` shows it.
    let mut body = String::new();
    let width = end.to_string().len();
    let (mut hash, mut author, mut time, mut line_no) =
        (String::new(), String::new(), 0i64, 0usize);
    let mut group = String::new();
    for line in porcelain.lines() {
        if let Some(content) = line.strip_prefix('\t') {
            let commit = if hash.bytes().all(|b| b == b'0') {
                "uncommitted".to_owned()
            } else {
                hash.chars().take(10).collect()
            };
            let header = format!("commit {commit} {} {author}\n", date_of(time));
            if header != group {
                body.push_str(&header);
                group = header;
            }
            body.push_str(&format!("{line_no:>width$}  {content}\n"));
        } else if let Some(a) = line.strip_prefix("author ") {
            author = a.chars().take(60).collect();
        } else if let Some(t) = line.strip_prefix("author-time ") {
            time = t.parse().unwrap_or(0);
        } else {
            let mut parts = line.split(' ');
            if let (Some(h), Some(_), Some(n)) = (parts.next(), parts.next(), parts.next())
                && h.len() >= 40
                && h.bytes().all(|b| b.is_ascii_hexdigit())
            {
                hash = h.to_owned();
                line_no = n.parse().unwrap_or(0);
            }
        }
    }
    let mut out = format!("{} lines {start}-{end} of {total}\n", path.display());
    out.push_str(&history(ctx, "HEAD", Some(&path), &body));
    if end < asked_end.min(total) {
        out.push_str(&format!(
            "[at most {MAX_BLAME_LINES} lines per call; continue from line {}]\n",
            end + 1
        ));
    }
    Ok(out)
}

/// `YYYY-MM-DD` of a Unix time (UTC).
fn date_of(unix: i64) -> String {
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = unix.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Whether the run (or session) in `run_dir` has made a commit.
pub(crate) fn has_committed(run_dir: &Path) -> bool {
    run_dir.join(BASE_FILE).exists()
}

/// The commit `diff` compares with: the run's base once it has committed, else HEAD.
pub(crate) fn diff_base(ctx: &Ctx<'_>) -> String {
    match std::fs::read_to_string(ctx.run_dir.join(BASE_FILE)) {
        Ok(base) if !base.trim().is_empty() => base.trim().to_owned(),
        // The repository had no commit when the run started: the empty tree.
        Ok(_) => run_text(ctx, &["hash-object", "-t", "tree", "/dev/null"])
            .map(|s| s.trim().to_owned())
            .unwrap_or_else(|_| "HEAD".into()),
        Err(_) => "HEAD".into(),
    }
}

/// Files this run wrote (its write journal, across resumes).
fn written(ctx: &Ctx<'_>) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = crate::journal::written(ctx.run_dir)
        .into_iter()
        .map(|w| w.path)
        .collect();
    for p in ctx.journal.paths() {
        if !paths.contains(&p) {
            paths.push(p);
        }
    }
    paths
}

/// Why `path` may never be committed by the frontier, if it may not.
fn refusal(ctx: &Ctx<'_>, path: &Path) -> Option<&'static str> {
    if declass_fs::is_reserved(path) || !ctx.presenter.path_visible(path) {
        Some("it is not available")
    } else if ctx.presenter.protection(path).is_some() {
        Some("it is protected source")
    } else if ctx.presenter.path_sensitive(path) {
        Some("it is sensitive (or derived from sensitive data)")
    } else {
        None
    }
}

fn commit(
    ctx: &mut Ctx<'_>,
    tools: &GitTools,
    args: &Map<String, Value>,
) -> Result<String, String> {
    let message = string_arg(args, "message")?.trim();
    if message.is_empty() {
        return Err("the commit message is empty".into());
    }
    if message.chars().count() > MAX_MESSAGE_CHARS {
        return Err(format!(
            "the commit message is longer than {MAX_MESSAGE_CHARS} characters"
        ));
    }
    if message.contains('⟨') || message.contains('⟩') {
        return Err(
            "the commit message contains a placeholder; placeholders are never resolved into history. \
Describe the change without the value."
                .into(),
        );
    }
    // What the boundary would show of the message: anything it replaces (a
    // known sensitive value, a secret, personal data, copied sensitive text)
    // must not become permanent history.
    let seen = history(ctx, "commit message", None, message);
    let _ = ctx.presenter.take_view_class();
    if seen != message {
        return Err(
            "the commit message contains a sensitive value (a secret, personal data or sensitive content); \
it was not committed. Describe the change without it."
                .into(),
        );
    }
    let written = written(ctx);
    let explicit = args.get("paths").and_then(Value::as_array);
    let mut skipped = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    match explicit {
        Some(list) => {
            for v in list {
                let raw = v.as_str().ok_or("`paths` must hold strings")?;
                let rel = declass_fs::normalize_relative(raw).map_err(fs_err)?;
                if !written.contains(&rel) {
                    return Err(format!(
                        "{raw} was not written in this run; git_commit commits only files you wrote"
                    ));
                }
                if let Some(why) = refusal(ctx, &rel) {
                    return Err(format!("{raw} may not be committed: {why}"));
                }
                paths.push(rel.to_string_lossy().into_owned());
            }
        }
        None => {
            for rel in &written {
                match refusal(ctx, rel) {
                    Some(why) => skipped.push(format!("{} ({why})", rel.display())),
                    None => paths.push(rel.to_string_lossy().into_owned()),
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    let blockers = ctx
        .git
        .commit_blockers(ctx.workspace, &paths)
        .map_err(git_err)?;
    if !blockers.is_empty() {
        if explicit.is_some() {
            let (p, why) = &blockers[0];
            return Err(format!("{p} may not be committed: {why}"));
        }
        for (p, why) in &blockers {
            skipped.push(format!("{p} ({why})"));
        }
        paths.retain(|p| !blockers.iter().any(|(b, _)| b == p));
    }
    if paths.is_empty() {
        let mut e = "nothing to commit: this run wrote no file that may be committed".to_owned();
        if !skipped.is_empty() {
            e.push_str(&format!(" (left out: {})", skipped.join(", ")));
        }
        return Err(e);
    }
    let author = tools
        .author
        .clone()
        .or_else(|| {
            ctx.git
                .operator_identity(ctx.workspace, &tools.user_files)
        })
        .ok_or(
            "no operator identity for the commit: the operator must set git.author, or user.name and \
user.email in git configuration",
        )?;
    // The base `diff` compares with, recorded once before the run's first commit.
    let base = ctx.run_dir.join(BASE_FILE);
    let first = !base.exists();
    if first {
        let head = ctx.git.head(ctx.workspace).map_err(git_err)?;
        declass_fs::private::write_private(&base, head.unwrap_or_default().as_bytes())
            .map_err(fs_err)?;
    }
    let index = ctx.run_dir.join("commit.index");
    let done = ctx
        .git
        .commit_paths(ctx.workspace, &index, &paths, message, &author)
        .map_err(|e| {
            if first {
                let _ = std::fs::remove_file(&base);
            }
            format!("the commit was not made: {e}")
        })?;
    ctx.record(AuditEvent::GitCommit {
        hash: done.hash.clone(),
        paths: paths.clone(),
    });
    let mut out = format!(
        "committed {} on {} ({} file{}): {}\n",
        &done.hash[..done.hash.len().min(12)],
        done.branch.as_deref().unwrap_or("detached HEAD"),
        paths.len(),
        if paths.len() == 1 { "" } else { "s" },
        paths.join(", ")
    );
    if !skipped.is_empty() {
        out.push_str(&format!("not committed: {}\n", skipped.join(", ")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_revisions() {
        assert_eq!(date_of(0), "1970-01-01");
        assert_eq!(date_of(1_790_000_000), "2026-09-21");
        for bad in ["", "-p", "--all", "a b", "x\ny"] {
            assert!(check_rev(bad).is_err(), "{bad:?}");
        }
        for ok in ["HEAD~2", "main..feature", "v1.0", "abc1234"] {
            assert!(check_rev(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn policies_parse() {
        assert_eq!("ask".parse(), Ok(CommitPolicy::Ask));
        assert_eq!("allow".parse(), Ok(CommitPolicy::Allow));
        assert_eq!("off".parse(), Ok(CommitPolicy::Off));
        assert!("yes".parse::<CommitPolicy>().is_err());
        assert_eq!(CommitPolicy::default(), CommitPolicy::Ask);
    }
}
