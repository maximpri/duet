// SPDX-License-Identifier: GPL-3.0-or-later
//! Language-server tools: `code_nav` (definition, references, hover,
//! symbols, workspace symbols, diagnostics) and `rename`, and the diagnostics
//! appended to edits of a file whose server is running.
//!
//! The boundary, whatever the server says:
//! - Servers run sandboxed with the paths checks may not read denied
//!   (sensitive files, `.git`, `.declass`; protected source stays readable, as
//!   it must be to build), and a server started before a path became hidden
//!   is restarted without it. Only files the frontier may see the content of
//!   are ever sent to a server.
//! - Every answer is shown as a list of locations (`path:line:col`) next to
//!   text presented as [`Source::CodeNav`] of the file it comes from, so a
//!   sensitive or sealed file shows no content and an interface-only file
//!   shows declarations only. Paths the frontier may not see are dropped.
//!   Positions inside protected source cannot be asked about at all.
//! - `rename` writes through the same path as `edit_file` (journal,
//!   preconditions, `resolve_for_write`) and is refused as a whole if any edit
//!   touches a file that is hidden, sensitive, protected, outside the
//!   repository, or not a source or test file.
//! - Every call and every server start, crash or restart is an audit event.

use crate::oversight::is_source_or_test;
use crate::tools::{Ctx, MAX_READ_BYTES, fs_err, string_arg};
use declass_boundary::audit::AuditEvent;
use declass_boundary::model::ToolSpec;
use declass_boundary::view::{CODE_NAV_WITHHELD, Presenter, Source};
use declass_fs::Precondition;
use declass_lsp::position::{from_lsp, line_text, offset, to_lsp};
use declass_lsp::{CallError, Diagnostic, Lsp, LspError, Position, Range};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Locations listed per answer.
const MAX_LOCATIONS: usize = 50;
/// Symbols listed per answer.
const MAX_SYMBOLS: usize = 150;
const MAX_HOVER_CHARS: usize = 4000;
/// Characters of a source line shown next to a location.
const SNIPPET_CHARS: usize = 160;
/// Diagnostics listed per file.
const MAX_DIAGNOSTICS: usize = 10;
const MESSAGE_CHARS: usize = 200;
/// Files whose diagnostics a rename waits for.
const RENAME_DIAGNOSTIC_FILES: usize = 5;

/// The language servers of a run, or `None` when they are turned off or none
/// is installed. Servers start in the OS sandbox on first use.
pub fn language_servers(
    settings: &declass_lsp::Settings,
    workspace: &Path,
    run_dir: &Path,
    sandbox: declass_sandbox::SandboxKind,
) -> Option<Lsp> {
    if !settings.enabled {
        return None;
    }
    let found = declass_lsp::servers::detect(settings, std::env::var_os("PATH").as_deref());
    if found.is_empty() {
        return None;
    }
    let root = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let run = run_dir
        .file_name()
        .map_or_else(|| "run".into(), |n| n.to_string_lossy().into_owned());
    let launcher = declass_lsp::SandboxLauncher {
        kind: sandbox,
        workspace: root.clone(),
        scratch: std::env::temp_dir()
            .join("declass-scratch")
            .join(format!("{run}-lsp")),
    };
    Some(Lsp::new(root, found, Box::new(launcher), settings.clone()))
}

/// The two tools, offered when the run has language servers.
pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "code_nav".into(),
            description: "Ask the language server about the code. `op`: definition, references or hover (need \
`path`, `line` and `column`: 1-based, lines as read_file numbers them, columns counting characters), symbols (the \
declarations in `path`), workspace_symbols (declarations whose name matches `query`; `path` optional, it picks the \
language), diagnostics (errors and warnings in `path`). Answers are `path:line:col  text` lines."
                .into(),
            parameters: json!({"type": "object", "properties": {
                "op": {"type": "string", "enum": ["definition", "references", "hover", "symbols",
                    "workspace_symbols", "diagnostics"]},
                "path": {"type": "string"},
                "line": {"type": "integer", "minimum": 1},
                "column": {"type": "integer", "minimum": 1},
                "query": {"type": "string"}
            }, "required": ["op"]}),
        },
        ToolSpec {
            name: "rename".into(),
            description: "Rename the symbol at `path`:`line`:`column` (1-based) everywhere the language server finds \
it. All edits are applied together or none; only source and test files may change. The result lists the files \
changed and their diagnostics."
                .into(),
            parameters: json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "line": {"type": "integer", "minimum": 1},
                "column": {"type": "integer", "minimum": 1},
                "new_name": {"type": "string"}
            }, "required": ["path", "line", "column", "new_name"]}),
        },
    ]
}

/// Why a call did not answer.
enum Fail {
    /// Refused by the boundary or invalid; the message says why.
    Refused(String),
    Lsp(LspError),
}

impl From<String> for Fail {
    fn from(s: String) -> Self {
        Fail::Refused(s)
    }
}

impl From<LspError> for Fail {
    fn from(e: LspError) -> Self {
        Fail::Lsp(e)
    }
}

impl Fail {
    fn outcome(&self) -> &'static str {
        match self {
            Fail::Refused(_) | Fail::Lsp(LspError::NoServer(_)) => "refused",
            Fail::Lsp(LspError::Unavailable { .. }) => "unavailable",
            Fail::Lsp(LspError::Call(CallError::Timeout)) => "timeout",
            Fail::Lsp(_) => "error",
        }
    }

    fn message(self) -> String {
        match self {
            Fail::Refused(s) => s,
            Fail::Lsp(e) => e.to_string(),
        }
    }
}

/// What a call showed, for the result and the audit event.
#[derive(Default)]
struct Answer {
    text: String,
    language: String,
    shown: u32,
    withheld: u32,
}

/// Paths a server may not read: what checks may not read, plus declass's own
/// state, git history and the sensitive commands' `TMPDIR` in every mode.
fn server_hidden(presenter: &dyn Presenter, workspace: &Path, run_dir: &Path) -> Vec<PathBuf> {
    let mut v = presenter.hidden_from_checks(workspace);
    v.push(workspace.join(".git"));
    v.push(workspace.join(".declass"));
    v.push(crate::tools::sensitive_scratch(run_dir));
    v
}

/// Protected source: hidden from commands but readable to checks.
fn protected_paths(presenter: &dyn Presenter, workspace: &Path) -> Vec<PathBuf> {
    let checks = presenter.hidden_from_checks(workspace);
    presenter
        .hidden_from_commands(workspace)
        .into_iter()
        .filter(|p| !checks.contains(p))
        .collect()
}

/// Where the frontier may look, decided once per call.
struct Scope<'a> {
    workspace: &'a Path,
    root: &'a Path,
    presenter: &'a dyn Presenter,
    protected: Vec<PathBuf>,
    /// File texts read for snippets, by relative path (`None`: not readable).
    texts: HashMap<PathBuf, Option<String>>,
}

impl<'a> Scope<'a> {
    fn new(ctx: &Ctx<'a>, lsp: &'a Lsp) -> Self {
        Self {
            workspace: ctx.workspace,
            root: lsp.root(),
            presenter: ctx.presenter,
            protected: protected_paths(ctx.presenter, ctx.workspace),
            texts: HashMap::new(),
        }
    }

    fn protected(&self, rel: &Path) -> bool {
        let abs = self.workspace.join(rel);
        self.protected.iter().any(|p| abs.starts_with(p))
    }

    /// The workspace-relative path of a server location, if inside the workspace.
    fn relative(&self, abs: &Path) -> Option<PathBuf> {
        abs.strip_prefix(self.root)
            .ok()
            .or_else(|| abs.strip_prefix(self.workspace).ok())
            .map(Path::to_path_buf)
    }

    /// The text of a visible file whose content the frontier may be shown in
    /// some form (never read for a sensitive file).
    fn text(&mut self, rel: &Path) -> Option<&str> {
        if !self.texts.contains_key(rel) {
            let text = if self.presenter.path_sensitive(rel) {
                None
            } else {
                declass_fs::read_file(self.workspace, rel, MAX_READ_BYTES)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
            };
            self.texts.insert(rel.to_path_buf(), text);
        }
        self.texts.get(rel).and_then(Option::as_deref)
    }

    /// 1-based line and column of a position in `rel`.
    fn place(&mut self, rel: &Path, pos: Position) -> (u32, u32) {
        match self.text(rel) {
            Some(t) => from_lsp(t, pos),
            None => (pos.line + 1, pos.character + 1),
        }
    }

    fn present(&self, rel: &Path, signature: bool, text: &str) -> String {
        self.presenter.present(
            &Source::CodeNav {
                path: rel.to_path_buf(),
                signature,
            },
            text.as_bytes(),
        )
    }

    /// One `path:line:col  view` line (the location is left out when a
    /// protected file's content is withheld). `None` if the path is hidden.
    fn line(
        &mut self,
        rel: &Path,
        pos: Position,
        signature: bool,
        text: &str,
        a: &mut Answer,
    ) -> Option<String> {
        if !self.presenter.path_visible(rel) {
            a.withheld += 1;
            return None;
        }
        let (line, col) = self.place(rel, pos);
        let view = self.present(rel, signature, text);
        let withheld = view.starts_with(CODE_NAV_WITHHELD);
        if withheld {
            a.withheld += 1;
        } else {
            a.shown += 1;
        }
        Some(if withheld && self.protected(rel) {
            format!("{}  {view}", rel.display())
        } else {
            format!("{}:{line}:{col}  {view}", rel.display())
        })
    }

    /// The trimmed source line at `pos` (empty for a file not read).
    fn snippet(&mut self, rel: &Path, pos: Position) -> String {
        self.text(rel)
            .and_then(|t| line_text(t, pos.line + 1))
            .map(|l| l.trim().chars().take(SNIPPET_CHARS).collect())
            .unwrap_or_default()
    }
}

/// A location outside the workspace (a dependency, the standard library):
/// the last path components and the line, never the text.
fn outside(abs: &Path, pos: Position) -> String {
    let parts: Vec<String> = abs
        .components()
        .rev()
        .take(3)
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let short: Vec<&str> = parts.iter().rev().map(String::as_str).collect();
    format!(
        "[outside the repository] …/{}:{}",
        short.join("/"),
        pos.line + 1
    )
}

fn file_path(uri: &str) -> Option<PathBuf> {
    url::Url::parse(uri).ok()?.to_file_path().ok()
}

fn position(v: &Value) -> Option<Position> {
    serde_json::from_value(v.clone()).ok()
}

/// The locations of a definition or references answer (`Location`,
/// `Location[]`, `LocationLink[]` or null).
fn locations(v: &Value) -> Vec<(PathBuf, Position)> {
    let items: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![v],
        _ => Vec::new(),
    };
    let mut out: Vec<(PathBuf, Position)> = items
        .into_iter()
        .filter_map(|l| {
            let (uri, start) = match l.get("targetUri") {
                Some(u) => (
                    u,
                    l.pointer("/targetSelectionRange/start")
                        .or_else(|| l.pointer("/targetRange/start"))?,
                ),
                None => (l.get("uri")?, l.pointer("/range/start")?),
            };
            Some((file_path(uri.as_str()?)?, position(start)?))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn symbol_kind(k: u64) -> &'static str {
    const KINDS: [&str; 26] = [
        "file",
        "module",
        "namespace",
        "package",
        "class",
        "method",
        "property",
        "field",
        "constructor",
        "enum",
        "interface",
        "function",
        "variable",
        "constant",
        "string",
        "number",
        "boolean",
        "array",
        "object",
        "key",
        "null",
        "enum member",
        "struct",
        "event",
        "operator",
        "type parameter",
    ];
    k.checked_sub(1)
        .and_then(|i| KINDS.get(i as usize))
        .copied()
        .unwrap_or("symbol")
}

/// Hover contents (`MarkupContent`, `MarkedString` or an array of them) as
/// plain lines, without code fences or rules.
fn hover_text(v: &Value) -> String {
    fn parts(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(a) => a.iter().for_each(|x| parts(x, out)),
            Value::Object(o) => {
                if let Some(s) = o.get("value").and_then(Value::as_str) {
                    out.push(s.to_owned());
                }
            }
            _ => {}
        }
    }
    let mut raw = Vec::new();
    parts(v.get("contents").unwrap_or(&Value::Null), &mut raw);
    let text: Vec<&str> = raw
        .iter()
        .flat_map(|p| p.lines())
        .filter(|l| !l.trim_start().starts_with("```") && l.trim() != "---")
        .collect();
    let text = text.join("\n").trim().to_owned();
    if text.chars().count() > MAX_HOVER_CHARS {
        let cut: String = text.chars().take(MAX_HOVER_CHARS).collect();
        format!("{cut}\n[hover text truncated]")
    } else {
        text
    }
}

/// A file the frontier asked about: visible, not sensitive, handled by a
/// server; `positions` also excludes protected source.
struct Target {
    rel: PathBuf,
    text: String,
    language: String,
}

fn target(
    ctx: &Ctx<'_>,
    lsp: &Lsp,
    scope: &Scope<'_>,
    raw: &str,
    positions: bool,
) -> Result<Target, Fail> {
    let rel = declass_fs::normalize_relative(raw).map_err(fs_err)?;
    if declass_fs::is_reserved(&rel) || !ctx.presenter.path_visible(&rel) {
        return Err(Fail::Refused(format!("{raw} is not available")));
    }
    if ctx.presenter.path_sensitive(&rel) {
        return Err(Fail::Refused(format!(
            "{raw} is sensitive: language servers never read it (ask_local answers questions about it)"
        )));
    }
    if positions && scope.protected(&rel) {
        return Err(Fail::Refused(format!(
            "{raw} is protected source: its code cannot be navigated; read its interface with read_file"
        )));
    }
    let language = lsp
        .server_for(&rel)
        .map(|s| s.spec.language.clone())
        .ok_or_else(|| {
            Fail::Lsp(LspError::NoServer(format!(
                "{raw} (no language server for this file type)"
            )))
        })?;
    let bytes = declass_fs::read_file(ctx.workspace, &rel, MAX_READ_BYTES).map_err(fs_err)?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{raw} is not valid UTF-8"))?;
    Ok(Target {
        rel,
        text,
        language,
    })
}

fn line_col(args: &Map<String, Value>, text: &str) -> Result<Position, String> {
    let n = |k: &str| {
        args.get(k)
            .and_then(Value::as_u64)
            .map(|v| v as u32)
            .ok_or_else(|| format!("`{k}` is required (1-based)"))
    };
    to_lsp(text, n("line")?, n("column")?)
}

fn at(pos: Position) -> impl Fn(&str) -> Value + Send + Sync {
    move |uri: &str| {
        json!({"textDocument": {"uri": uri}, "position": pos,
               "context": {"includeDeclaration": true}})
    }
}

/// Records the call and any server lifecycle events.
fn audit(
    ctx: &Ctx<'_>,
    lsp: &Lsp,
    op: &str,
    path: Option<&str>,
    language: &str,
    outcome: &str,
    a: Option<&Answer>,
) {
    for e in lsp.take_events() {
        ctx.record(AuditEvent::LanguageServer {
            language: e.language,
            op: "server".into(),
            path: None,
            outcome: e.what,
            shown: 0,
            withheld: 0,
        });
    }
    ctx.record(AuditEvent::LanguageServer {
        language: language.to_owned(),
        op: op.to_owned(),
        path: path.map(str::to_owned),
        outcome: outcome.to_owned(),
        shown: a.map_or(0, |a| a.shown),
        withheld: a.map_or(0, |a| a.withheld),
    });
}

pub(crate) async fn code_nav(
    ctx: &mut Ctx<'_>,
    args: &Map<String, Value>,
) -> Result<String, String> {
    let lsp = ctx.lsp.ok_or("language servers are off for this run")?;
    let op = string_arg(args, "op")?.to_owned();
    let path = args.get("path").and_then(Value::as_str).map(str::to_owned);
    let result = navigate(ctx, lsp, &op, args).await;
    let (language, outcome, answer) = match &result {
        Ok(a) => (a.language.clone(), "ok", Some(a)),
        Err(f) => (String::new(), f.outcome(), None),
    };
    // A refused path is not recorded: it may be one the frontier cannot see.
    let recorded = path.filter(|_| outcome != "refused");
    audit(
        ctx,
        lsp,
        &format!("code_nav:{op}"),
        recorded.as_deref(),
        &language,
        outcome,
        answer,
    );
    match result {
        Ok(a) => Ok(a.text),
        Err(f) => Err(f.message()),
    }
}

async fn navigate(
    ctx: &Ctx<'_>,
    lsp: &Lsp,
    op: &str,
    args: &Map<String, Value>,
) -> Result<Answer, Fail> {
    let presenter = ctx.presenter;
    let ws = ctx.workspace;
    let run_dir = ctx.run_dir;
    let hidden = move || server_hidden(presenter, ws, run_dir);
    let mut scope = Scope::new(ctx, lsp);
    let raw_path = args.get("path").and_then(Value::as_str);
    let need_path =
        || raw_path.ok_or_else(|| Fail::Refused("`path` is required for this op".into()));
    match op {
        "definition" | "references" | "hover" => {
            let t = target(ctx, lsp, &scope, need_path()?, true)?;
            let pos = line_col(args, &t.text)?;
            let def = lsp
                .request(
                    &t.rel,
                    &t.text,
                    &hidden,
                    "textDocument/definition",
                    &at(pos),
                )
                .await?;
            let mut a = Answer {
                language: t.language.clone(),
                ..Answer::default()
            };
            if op == "hover" {
                let v = lsp
                    .request(&t.rel, &t.text, &hidden, "textDocument/hover", &at(pos))
                    .await?;
                let text = hover_text(&v);
                if text.is_empty() {
                    a.text = "no hover information at that position".into();
                    return Ok(a);
                }
                // Hover text describes the symbol's declaration: it is shown
                // as the declaring file would be (the asked file if unknown).
                let declared = locations(&def)
                    .into_iter()
                    .find_map(|(abs, _)| scope.relative(&abs))
                    .unwrap_or_else(|| t.rel.clone());
                if !presenter.path_visible(&declared) {
                    a.withheld = 1;
                    a.text = format!("{CODE_NAV_WITHHELD} declared in a file you cannot see⟩");
                    return Ok(a);
                }
                let view = scope.present(&declared, true, &text);
                if view.starts_with(CODE_NAV_WITHHELD) {
                    a.withheld = 1;
                } else {
                    a.shown = 1;
                }
                a.text = format!("declared in {}\n{view}", declared.display());
                return Ok(a);
            }
            let found = if op == "definition" {
                locations(&def)
            } else {
                let v = lsp
                    .request(
                        &t.rel,
                        &t.text,
                        &hidden,
                        "textDocument/references",
                        &at(pos),
                    )
                    .await?;
                locations(&v)
            };
            let mut lines = Vec::new();
            for (abs, pos) in &found {
                if lines.len() >= MAX_LOCATIONS {
                    break;
                }
                match scope.relative(abs) {
                    None => {
                        a.shown += 1;
                        lines.push(outside(abs, *pos));
                    }
                    Some(rel) => {
                        let snippet = if presenter.path_visible(&rel) {
                            scope.snippet(&rel, *pos)
                        } else {
                            String::new()
                        };
                        if let Some(l) = scope.line(&rel, *pos, false, &snippet, &mut a) {
                            lines.push(l);
                        }
                    }
                }
            }
            let listed = a.shown + a.withheld;
            if found.len() > listed as usize {
                lines.push(format!(
                    "[{} more not shown]",
                    found.len() - listed as usize
                ));
            }
            a.text = if lines.is_empty() {
                format!("no {op} found")
            } else {
                lines.join("\n")
            };
            Ok(a)
        }
        "symbols" => {
            let t = target(ctx, lsp, &scope, need_path()?, false)?;
            let v = lsp
                .request(
                    &t.rel,
                    &t.text,
                    &hidden,
                    "textDocument/documentSymbol",
                    &|uri: &str| json!({"textDocument": {"uri": uri}}),
                )
                .await?;
            let mut rows = Vec::new();
            flatten_symbols(&v, 0, &mut rows);
            let total = rows.len();
            let mut listing = String::new();
            for (depth, pos, kind, name, detail) in rows.into_iter().take(MAX_SYMBOLS) {
                let (line, col) = from_lsp(&t.text, pos);
                let detail = if detail.is_empty() {
                    String::new()
                } else {
                    format!(
                        "  {}",
                        detail.chars().take(SNIPPET_CHARS).collect::<String>()
                    )
                };
                listing.push_str(&format!(
                    "{}{line}:{col}  {kind} {name}{detail}\n",
                    "  ".repeat(depth)
                ));
            }
            if total > MAX_SYMBOLS {
                listing.push_str(&format!("[{} more not shown]\n", total - MAX_SYMBOLS));
            }
            let mut a = Answer {
                language: t.language,
                ..Answer::default()
            };
            if total == 0 {
                a.text = format!("no symbols in {}", t.rel.display());
                return Ok(a);
            }
            let view = scope.present(&t.rel, true, &listing);
            if view.starts_with(CODE_NAV_WITHHELD) {
                a.withheld = total as u32;
            } else {
                a.shown = total.min(MAX_SYMBOLS) as u32;
            }
            a.text = format!("{}\n{view}", t.rel.display());
            Ok(a)
        }
        "workspace_symbols" => {
            let query = string_arg(args, "query")?.to_owned();
            let language = match raw_path {
                Some(p) => target(ctx, lsp, &scope, p, false)?.language,
                None => workspace_language(ctx, lsp).await?,
            };
            let v = lsp
                .workspace_request(
                    &language,
                    &hidden,
                    "workspace/symbol",
                    json!({"query": query}),
                )
                .await?;
            let mut a = Answer {
                language,
                ..Answer::default()
            };
            let items = v.as_array().cloned().unwrap_or_default();
            let mut lines = Vec::new();
            for s in &items {
                if lines.len() >= MAX_SYMBOLS {
                    break;
                }
                let name = s.get("name").and_then(Value::as_str).unwrap_or_default();
                let kind = symbol_kind(s.get("kind").and_then(Value::as_u64).unwrap_or(0));
                let container = s
                    .get("containerName")
                    .and_then(Value::as_str)
                    .filter(|c| !c.is_empty())
                    .map(|c| format!(" (in {c})"))
                    .unwrap_or_default();
                let Some(abs) = s
                    .pointer("/location/uri")
                    .and_then(Value::as_str)
                    .and_then(file_path)
                else {
                    continue;
                };
                let pos = s
                    .pointer("/location/range/start")
                    .and_then(position)
                    .unwrap_or(Position {
                        line: 0,
                        character: 0,
                    });
                let text = format!("{kind} {name}{container}");
                match scope.relative(&abs) {
                    None => {
                        a.shown += 1;
                        lines.push(format!("{}  {text}", outside(&abs, pos)));
                    }
                    Some(rel) => {
                        if let Some(l) = scope.line(&rel, pos, true, &text, &mut a) {
                            lines.push(l);
                        }
                    }
                }
            }
            if items.len() > (a.shown + a.withheld) as usize {
                lines.push(format!(
                    "[{} more not shown; narrow the query]",
                    items.len() - (a.shown + a.withheld) as usize
                ));
            }
            a.text = if lines.is_empty() {
                "no matching symbols".into()
            } else {
                lines.join("\n")
            };
            Ok(a)
        }
        "diagnostics" => {
            let t = target(ctx, lsp, &scope, need_path()?, true)?;
            let wait = lsp
                .settings
                .diagnostics_wait
                .max(Duration::from_millis(500));
            let published = lsp
                .diagnostics(&t.rel, &t.text, &hidden, true, wait)
                .await?;
            let mut a = Answer {
                language: t.language,
                ..Answer::default()
            };
            a.text = match published {
                Some(p) => {
                    a.shown = p.items.len() as u32;
                    render_diagnostics(&scope, &t.rel, &t.text, &p.items)
                }
                None => format!(
                    "diagnostics: none reported within {:.1}s (the server may still be analysing)",
                    wait.as_secs_f64()
                ),
            };
            Ok(a)
        }
        other => Err(Fail::Refused(format!(
            "unknown op `{other}`; use definition, references, hover, symbols, workspace_symbols or diagnostics"
        ))),
    }
}

/// (depth, position, kind, name, detail) of each symbol, depth-first.
fn flatten_symbols(
    v: &Value,
    depth: usize,
    out: &mut Vec<(usize, Position, &'static str, String, String)>,
) {
    for s in v.as_array().into_iter().flatten() {
        let pos = s
            .pointer("/selectionRange/start")
            .or_else(|| s.pointer("/location/range/start"))
            .or_else(|| s.pointer("/range/start"))
            .and_then(position)
            .unwrap_or(Position {
                line: 0,
                character: 0,
            });
        let text = |k: &str| {
            s.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned()
        };
        out.push((
            depth.min(6),
            pos,
            symbol_kind(s.get("kind").and_then(Value::as_u64).unwrap_or(0)),
            text("name"),
            text("detail"),
        ));
        if let Some(children) = s.get("children") {
            flatten_symbols(children, depth + 1, out);
        }
    }
}

/// The language to ask about the whole workspace: a running server, else the
/// one handling the most files.
async fn workspace_language(ctx: &Ctx<'_>, lsp: &Lsp) -> Result<String, Fail> {
    for s in lsp.servers() {
        if lsp.active(&s.spec.language).await {
            return Ok(s.spec.language.clone());
        }
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for f in ctx.git.list_files(ctx.workspace).unwrap_or_default() {
        if let Some(s) = lsp.server_for(Path::new(&f)) {
            *counts.entry(s.spec.language.clone()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(l, _)| l)
        .ok_or_else(|| {
            Fail::Refused("no language server handles the files of this repository".into())
        })
}

fn severity_name(s: Option<u8>) -> &'static str {
    match s {
        Some(2) => "warning",
        Some(3) => "info",
        Some(4) => "hint",
        _ => "error",
    }
}

/// Errors first, then warnings, capped; presented as the file's content.
fn render_diagnostics(scope: &Scope<'_>, rel: &Path, text: &str, items: &[Diagnostic]) -> String {
    let rank = |d: &Diagnostic| d.severity.unwrap_or(1);
    let mut shown: Vec<&Diagnostic> = items.iter().filter(|d| rank(d) <= 2).collect();
    shown.sort_by_key(|d| (rank(d), d.range.start));
    let errors = shown.iter().filter(|d| rank(d) == 1).count();
    let warnings = shown.len() - errors;
    let others = items.len() - shown.len();
    if shown.is_empty() {
        return if others > 0 {
            format!("diagnostics: no errors or warnings ({others} note(s))")
        } else {
            "diagnostics: none".into()
        };
    }
    let mut body = String::new();
    for d in shown.iter().take(MAX_DIAGNOSTICS) {
        let (line, col) = from_lsp(text, d.range.start);
        let message: String = d
            .message
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(MESSAGE_CHARS)
            .collect();
        let source = d
            .source
            .as_deref()
            .map(|s| format!(" [{s}]"))
            .unwrap_or_default();
        body.push_str(&format!(
            "  {}:{line}:{col} {}: {message}{source}\n",
            rel.display(),
            severity_name(d.severity)
        ));
    }
    if shown.len() > MAX_DIAGNOSTICS {
        body.push_str(&format!(
            "  [{} more not shown]\n",
            shown.len() - MAX_DIAGNOSTICS
        ));
    }
    format!(
        "diagnostics: {errors} error(s), {warnings} warning(s)\n{}",
        scope.present(rel, false, &body).trim_end()
    )
}

/// `text` with the diagnostics of the file `edit_file` or `write_file` just
/// wrote appended, when a server for it is running (none is started for this).
pub(crate) async fn with_diagnostics(
    ctx: &Ctx<'_>,
    args: &Map<String, Value>,
    text: String,
) -> String {
    let Some(lsp) = ctx.lsp else { return text };
    let Some(rel) = args
        .get("path")
        .and_then(Value::as_str)
        .and_then(|p| declass_fs::writable_relative(p).ok())
    else {
        return text;
    };
    let Some(server) = lsp.server_for(&rel) else {
        return text;
    };
    let language = server.spec.language.clone();
    if !lsp.active(&language).await
        || !ctx.presenter.path_visible(&rel)
        || ctx.presenter.path_sensitive(&rel)
    {
        return text;
    }
    let Some(content) = declass_fs::read_file(ctx.workspace, &rel, MAX_READ_BYTES)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
    else {
        return text;
    };
    let scope = Scope::new(ctx, lsp);
    if scope.protected(&rel) {
        return text;
    }
    let presenter = ctx.presenter;
    let ws = ctx.workspace;
    let run_dir = ctx.run_dir;
    let hidden = move || server_hidden(presenter, ws, run_dir);
    let wait = lsp.settings.diagnostics_wait;
    let (section, outcome) = match lsp.diagnostics(&rel, &content, &hidden, false, wait).await {
        Ok(Some(p)) => (render_diagnostics(&scope, &rel, &content, &p.items), "ok"),
        Ok(None) => (
            format!(
                "diagnostics: none reported within {:.1}s",
                wait.as_secs_f64()
            ),
            "ok",
        ),
        Err(e) => (format!("diagnostics: {e}"), Fail::Lsp(e).outcome()),
    };
    let rel_s = rel.display().to_string();
    audit(
        ctx,
        lsp,
        "diagnostics_after_edit",
        Some(&rel_s),
        &language,
        outcome,
        None,
    );
    format!("{text}\n{section}")
}

/// The name at a 1-based position of `text`.
fn name_at(text: &str, line: u32, column: u32) -> Option<String> {
    let chars: Vec<char> = line_text(text, line)?.chars().collect();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let i = (column as usize).checked_sub(1)?;
    if !chars.get(i).is_some_and(|c| word(*c)) {
        return None;
    }
    let start = (0..=i).rev().take_while(|&j| word(chars[j])).last()?;
    let end = (i..chars.len()).take_while(|&j| word(chars[j])).last()?;
    Some(chars[start..=end].iter().collect())
}

/// The text edits of a `WorkspaceEdit`, by file; resource operations refused.
fn text_edits(v: &Value) -> Result<BTreeMap<PathBuf, Vec<(Range, String)>>, String> {
    let mut out: BTreeMap<PathBuf, Vec<(Range, String)>> = BTreeMap::new();
    let mut add = |uri: &str, edits: &Value| -> Result<(), String> {
        let path = file_path(uri)
            .ok_or_else(|| format!("the language server named an invalid file {uri}"))?;
        for e in edits.as_array().into_iter().flatten() {
            let range: Range =
                serde_json::from_value(e.get("range").cloned().unwrap_or(Value::Null))
                    .map_err(|_| "the language server sent an edit without a range".to_owned())?;
            let text = e.get("newText").and_then(Value::as_str).unwrap_or_default();
            out.entry(path.clone())
                .or_default()
                .push((range, text.to_owned()));
        }
        Ok(())
    };
    if v.is_null() {
        return Err("the language server found nothing to rename at that position".into());
    }
    if let Some(changes) = v.get("documentChanges").and_then(Value::as_array) {
        for c in changes {
            if c.get("kind").is_some() {
                return Err(
                    "the rename would also create, rename or delete files; nothing was changed. \
Move files with write_file and run_command, then rename"
                        .into(),
                );
            }
            let uri = c
                .pointer("/textDocument/uri")
                .and_then(Value::as_str)
                .unwrap_or_default();
            add(uri, c.get("edits").unwrap_or(&Value::Null))?;
        }
    } else if let Some(changes) = v.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            add(uri, edits)?;
        }
    }
    Ok(out)
}

/// `text` with the edits applied (ranges must not overlap). Every replaced
/// range must hold `old` (the name renamed), or the server's view was stale.
fn apply(text: &str, edits: &[(Range, String)], old: &str) -> Result<String, String> {
    let mut spans = Vec::new();
    for (r, new) in edits {
        let (Some(s), Some(e)) = (offset(text, r.start), offset(text, r.end)) else {
            return Err("an edit lies outside the file".into());
        };
        if s > e {
            return Err("an edit has its end before its start".into());
        }
        let current = text[s..e].trim_start_matches("r#").trim_start_matches('\'');
        if s < e && current != old {
            return Err(
                "the language server's view of the file is out of date; nothing was changed. Retry the rename".into(),
            );
        }
        spans.push((s, e, new.as_str()));
    }
    spans.sort_by_key(|(s, e, _)| (*s, *e));
    if spans.windows(2).any(|w| w[0].1 > w[1].0) {
        return Err("the language server sent overlapping edits; nothing was changed".into());
    }
    let mut out = text.to_owned();
    for (s, e, new) in spans.into_iter().rev() {
        out.replace_range(s..e, new);
    }
    Ok(out)
}

pub(crate) async fn rename(ctx: &mut Ctx<'_>, args: &Map<String, Value>) -> Result<String, String> {
    let lsp = ctx.lsp.ok_or("language servers are off for this run")?;
    let path = args.get("path").and_then(Value::as_str).map(str::to_owned);
    let result = rename_in(ctx, lsp, args).await;
    let (language, outcome, answer) = match &result {
        Ok(a) => (a.language.clone(), "ok", Some(a)),
        Err(f) => (String::new(), f.outcome(), None),
    };
    let recorded = path.filter(|_| outcome != "refused");
    audit(
        ctx,
        lsp,
        "rename",
        recorded.as_deref(),
        &language,
        outcome,
        answer,
    );
    match result {
        Ok(a) => Ok(a.text),
        Err(f) => Err(f.message()),
    }
}

async fn rename_in(
    ctx: &mut Ctx<'_>,
    lsp: &Lsp,
    args: &Map<String, Value>,
) -> Result<Answer, Fail> {
    let new_name = string_arg(args, "new_name")?.to_owned();
    if new_name.trim().is_empty()
        || new_name.contains(['\n', '\r'])
        || new_name.chars().count() > 200
    {
        return Err(Fail::Refused(
            "`new_name` must be one line of up to 200 characters".into(),
        ));
    }
    let presenter = ctx.presenter;
    if presenter.detokenize(&new_name) != new_name {
        return Err(Fail::Refused(
            "`new_name` may not contain placeholders".into(),
        ));
    }
    let ws = ctx.workspace;
    let run_dir = ctx.run_dir;
    let hidden = move || server_hidden(presenter, ws, run_dir);
    let scope = Scope::new(ctx, lsp);
    let t = target(ctx, lsp, &scope, string_arg(args, "path")?, true)?;
    let pos = line_col(args, &t.text)?;
    let n = |k: &str| args.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
    let old = name_at(&t.text, n("line"), n("column"))
        .ok_or_else(|| Fail::Refused("that position is not on a name".into()))?;
    let v = lsp
        .request(&t.rel, &t.text, &hidden, "textDocument/rename", &|uri: &str| {
            json!({"textDocument": {"uri": uri}, "position": pos, "newName": new_name})
        })
        .await?;
    let by_file = text_edits(&v)?;
    if by_file.is_empty() {
        return Err(Fail::Refused(
            "the language server found nothing to rename at that position".into(),
        ));
    }
    // Check every file first: the rename is applied whole or not at all.
    let mut plan = Vec::new();
    let mut edits_total = 0;
    for (abs, edits) in &by_file {
        let Some(rel) = scope.relative(abs) else {
            return Err(Fail::Refused(
                "the rename reaches a file outside the repository (a dependency); nothing was changed".into(),
            ));
        };
        let why = if !presenter.path_visible(&rel) {
            Some("a file you cannot see".to_owned())
        } else if presenter.path_sensitive(&rel) {
            Some(format!("{} (sensitive)", rel.display()))
        } else if scope.protected(&rel) {
            Some(format!(
                "{} (protected source; use edit_protected)",
                rel.display()
            ))
        } else if declass_fs::writable_relative(&rel.display().to_string()).is_err()
            || !is_source_or_test(&rel, false)
        {
            Some(format!("{} (not a source or test file)", rel.display()))
        } else {
            None
        };
        if let Some(why) = why {
            return Err(Fail::Refused(format!(
                "the rename would change {why}; nothing was changed"
            )));
        }
        let bytes = declass_fs::read_file(ctx.workspace, &rel, MAX_READ_BYTES).map_err(fs_err)?;
        let original = String::from_utf8(bytes.clone())
            .map_err(|_| format!("{} is not valid UTF-8", rel.display()))?;
        let updated =
            apply(&original, edits, &old).map_err(|e| format!("{}: {e}", rel.display()))?;
        presenter.note_authored(&new_name);
        let updated = presenter.resolve_for_write(&rel, &updated)?;
        edits_total += edits.len();
        plan.push((rel, bytes, updated));
    }
    let mut written: Vec<(PathBuf, Vec<u8>, String)> = Vec::new();
    for (rel, original, updated) in plan {
        let pre = Precondition::Sha256(declass_fs::sha256_hex(&original));
        if let Err(e) = ctx
            .journal
            .write(ctx.workspace, &rel, updated.as_bytes(), &pre)
        {
            // Put back what was already written.
            for (r, o, u) in written.iter().rev() {
                let pre = Precondition::Sha256(declass_fs::sha256_hex(u.as_bytes()));
                let _ = ctx.journal.write(ctx.workspace, r, o, &pre);
            }
            return Err(Fail::Refused(format!(
                "{}: {}; nothing was changed",
                rel.display(),
                fs_err(e)
            )));
        }
        written.push((rel, original, updated));
    }
    let mut a = Answer {
        language: t.language,
        shown: edits_total as u32,
        ..Answer::default()
    };
    let mut out = format!(
        "renamed to `{new_name}`: {edits_total} edit(s) in {} file(s)\n",
        written.len()
    );
    for (rel, _, _) in &written {
        let count = by_file
            .iter()
            .find(|(abs, _)| scope.relative(abs).as_deref() == Some(rel.as_path()))
            .map_or(0, |(_, e)| e.len());
        out.push_str(&format!("  {} ({count})\n", rel.display()));
    }
    // The server sees the new text; the first files' diagnostics are shown.
    for (i, (rel, _, updated)) in written.iter().enumerate() {
        let wait = if i < RENAME_DIAGNOSTIC_FILES {
            lsp.settings.diagnostics_wait
        } else {
            Duration::ZERO
        };
        if let Ok(Some(p)) = lsp.diagnostics(rel, updated, &hidden, false, wait).await
            && i < RENAME_DIAGNOSTIC_FILES
            && p.items.iter().any(|d| d.severity.unwrap_or(1) <= 2)
        {
            out.push_str(&render_diagnostics(&scope, rel, updated, &p.items));
            out.push('\n');
        }
    }
    a.text = out.trim_end().to_owned();
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_apply_from_the_end_and_refuse_stale_or_overlapping_ranges() {
        let r = |l: u32, s: u32, e: u32| Range {
            start: Position {
                line: l,
                character: s,
            },
            end: Position {
                line: l,
                character: e,
            },
        };
        let text = "fn add(a: i32) {}\nlet x = add(1) + add(2);\n";
        let edits = vec![
            (r(1, 17, 20), "plus".to_owned()),
            (r(0, 3, 6), "plus".to_owned()),
            (r(1, 8, 11), "plus".to_owned()),
        ];
        assert_eq!(
            apply(text, &edits, "add").unwrap(),
            "fn plus(a: i32) {}\nlet x = plus(1) + plus(2);\n"
        );
        assert!(
            apply(text, &[(r(0, 0, 2), "x".into())], "add")
                .unwrap_err()
                .contains("out of date")
        );
        let overlap = vec![(r(0, 3, 6), "p".to_owned()), (r(0, 4, 5), "q".to_owned())];
        assert!(apply(text, &overlap, "add").is_err());
        // An insertion (empty range) needs no check.
        assert_eq!(
            apply("a", &[(r(0, 1, 1), ": b".into())], "zzz").unwrap(),
            "a: b"
        );
    }

    #[test]
    fn resource_operations_and_empty_answers_are_refused() {
        assert!(text_edits(&Value::Null).is_err());
        let with_create =
            json!({"documentChanges": [{"kind": "create", "uri": "file:///ws/x.rs"}]});
        assert!(
            text_edits(&with_create)
                .unwrap_err()
                .contains("create, rename or delete")
        );
        let changes = json!({"changes": {"file:///ws/a.rs": [
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "newText": "b"}]}});
        assert_eq!(text_edits(&changes).unwrap().len(), 1);
    }

    #[test]
    fn names_hover_text_and_symbol_kinds() {
        assert_eq!(
            name_at("let s = \"é😀\"; add_one(2);", 1, 17).as_deref(),
            Some("add_one")
        );
        assert_eq!(name_at("a + b", 1, 2), None);
        let hover = json!({"contents": {"kind": "markdown",
            "value": "```rust\nfn add_one(x: i32) -> i32\n```\n---\nAdds one."}});
        assert_eq!(hover_text(&hover), "fn add_one(x: i32) -> i32\nAdds one.");
        assert_eq!(
            hover_text(&json!({"contents": ["a", {"language": "c", "value": "int x"}]})),
            "a\nint x"
        );
        assert_eq!(symbol_kind(12), "function");
        assert_eq!(symbol_kind(99), "symbol");
    }
}

#[cfg(test)]
mod tool_tests {
    use super::*;
    use crate::journal::WriteJournal;
    use crate::tools::{Outcome, dispatch};
    use declass_boundary::view::PassThrough;
    use declass_lsp::mock::{MockLauncher, MockOptions, detected};

    const MAIN: &str = "fn main() {\n    let s = \"é😀\"; helper(1);\n}\n";
    const UTIL: &str = "pub fn helper(x: u32) {}\n";

    struct Fixture {
        _d: tempfile::TempDir,
        ws: PathBuf,
        run: PathBuf,
        lsp: Lsp,
        launcher: MockLauncher,
    }

    fn fixture(opts: MockOptions) -> Fixture {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        let (ws, run) = (root.join("ws"), root.join("run"));
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::create_dir_all(ws.join("docs")).unwrap();
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(ws.join("src/main.rs"), MAIN).unwrap();
        std::fs::write(ws.join("src/util.rs"), UTIL).unwrap();
        let launcher = MockLauncher {
            opts,
            ..MockLauncher::default()
        };
        let lsp = Lsp::new(
            ws.clone(),
            vec![detected("rust", &["rs"])],
            Box::new(launcher.clone()),
            declass_lsp::Settings {
                enabled: true,
                request_timeout: Duration::from_secs(5),
                diagnostics_wait: Duration::from_secs(2),
                ..declass_lsp::Settings::default()
            },
        );
        Fixture {
            _d: d,
            ws,
            run,
            lsp,
            launcher,
        }
    }

    async fn call(f: &Fixture, name: &str, args: Value) -> Result<String, String> {
        let git = declass_git::Git::locate().unwrap();
        let presenter = PassThrough { max_bytes: 60_000 };
        let mut journal = WriteJournal::open(&f.run).unwrap();
        let mut ctx = Ctx {
            workspace: &f.ws,
            run_dir: &f.run,
            sandbox: declass_sandbox::SandboxKind::Seatbelt,
            git: &git,
            presenter: &presenter,
            journal: &mut journal,
            command_timeout: Duration::from_secs(10),
            network: &crate::egress::Network::Off,
            checks: &[],
            audit: None,
            interrupted: None,
            web: None,
            git_tools: None,
            lsp: Some(&f.lsp),
        };
        let Value::Object(args) = args else {
            unreachable!()
        };
        match dispatch(&mut ctx, name, &args).await {
            Outcome::Result(s) => Ok(s),
            Outcome::Error(e) => Err(e),
            other => Err(format!("{other:?}")),
        }
    }

    #[tokio::test]
    async fn columns_count_characters_and_answers_come_back_in_them() {
        let f = fixture(MockOptions::default());
        // `helper` is the 19th character of line 2, after an emoji that takes
        // two UTF-16 units.
        let refs = call(
            &f,
            "code_nav",
            json!({"op": "references", "path": "src/main.rs", "line": 2, "column": 19}),
        )
        .await
        .unwrap();
        assert_eq!(
            refs,
            "src/main.rs:2:19  let s = \"é😀\"; helper(1);\nsrc/util.rs:1:8  pub fn helper(x: u32) {}"
        );
        let symbols = call(
            &f,
            "code_nav",
            json!({"op": "symbols", "path": "src/util.rs"}),
        )
        .await
        .unwrap();
        assert_eq!(
            symbols,
            "src/util.rs\n1:8  function helper  pub fn helper(x: u32) {}\n"
        );
        let err = call(
            &f,
            "code_nav",
            json!({"op": "hover", "path": "src/main.rs", "line": 9, "column": 1}),
        )
        .await
        .unwrap_err();
        assert!(err.contains("outside the file"), "{err}");
        let err = call(
            &f,
            "code_nav",
            json!({"op": "definition", "path": "README.md", "line": 1, "column": 1}),
        )
        .await
        .unwrap_err();
        assert!(err.contains("no language server"), "{err}");
    }

    #[tokio::test]
    async fn edits_carry_diagnostics_once_a_server_runs() {
        let f = fixture(MockOptions::default());
        let broken = json!({"path": "src/util.rs", "edits": [{"old": "{}", "new": "{ ERROR }"}]});
        // No server yet: none is started just for this.
        let plain = call(&f, "edit_file", broken.clone()).await.unwrap();
        assert_eq!(plain, "edited src/util.rs (1 edit)");
        assert_eq!(
            f.launcher
                .launches
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let d = call(
            &f,
            "code_nav",
            json!({"op": "diagnostics", "path": "src/util.rs"}),
        )
        .await
        .unwrap();
        assert!(
            d.starts_with("diagnostics: 1 error(s), 0 warning(s)"),
            "{d}"
        );
        assert!(
            d.contains("src/util.rs:1:25 error: ERROR marker here [mock]"),
            "{d}"
        );
        assert!(!d.contains("second line"), "one line per message: {d}");
        let fixed = call(
            &f,
            "write_file",
            json!({"path": "src/util.rs", "content": "pub fn helper(x: u32) {}\n// WARN\n"}),
        )
        .await
        .unwrap();
        assert!(fixed.ends_with("diagnostics: 0 error(s), 1 warning(s)\n  src/util.rs:2:4 warning: WARN marker here [mock]"), "{fixed}");
        // Files of other types get no section.
        let other = call(
            &f,
            "write_file",
            json!({"path": "docs/notes.md", "content": "ERROR\n"}),
        )
        .await
        .unwrap();
        assert!(!other.contains("diagnostics"), "{other}");
    }

    #[tokio::test]
    async fn a_rename_is_refused_whole_when_one_file_may_not_change() {
        let f = fixture(MockOptions {
            extensions: vec!["rs".into(), "md".into()],
            ..MockOptions::default()
        });
        std::fs::write(f.ws.join("docs/notes.md"), "call helper first\n").unwrap();
        let args = json!({"path": "src/util.rs", "line": 1, "column": 8, "new_name": "assist"});
        let err = call(&f, "rename", args.clone()).await.unwrap_err();
        assert!(
            err.contains("docs/notes.md (not a source or test file); nothing was changed"),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(f.ws.join("src/util.rs")).unwrap(),
            UTIL
        );
        assert_eq!(
            std::fs::read_to_string(f.ws.join("src/main.rs")).unwrap(),
            MAIN
        );
        std::fs::remove_file(f.ws.join("docs/notes.md")).unwrap();
        let err = call(
            &f,
            "rename",
            json!({"path": "src/util.rs", "line": 1, "column": 4, "new_name": "x"}),
        )
        .await
        .unwrap_err();
        assert!(err.contains("not on a name"), "{err}");
        let err = call(
            &f,
            "rename",
            json!({"path": "src/util.rs", "line": 1, "column": 8, "new_name": "a\nb"}),
        )
        .await
        .unwrap_err();
        assert!(err.contains("one line"), "{err}");
        let ok = call(&f, "rename", args).await.unwrap();
        assert!(
            ok.starts_with("renamed to `assist`: 2 edit(s) in 2 file(s)"),
            "{ok}"
        );
        assert!(
            std::fs::read_to_string(f.ws.join("src/main.rs"))
                .unwrap()
                .contains("assist(1)")
        );
    }

    #[tokio::test]
    async fn a_rename_that_creates_files_is_refused() {
        let f = fixture(MockOptions {
            resource_op: true,
            ..MockOptions::default()
        });
        let err = call(
            &f,
            "rename",
            json!({"path": "src/util.rs", "line": 1, "column": 8, "new_name": "assist"}),
        )
        .await
        .unwrap_err();
        assert!(err.contains("create, rename or delete files"), "{err}");
        assert_eq!(
            std::fs::read_to_string(f.ws.join("src/util.rs")).unwrap(),
            UTIL
        );
        // Versioned document edits are applied like plain ones.
        let f = fixture(MockOptions {
            document_changes: true,
            ..MockOptions::default()
        });
        let ok = call(
            &f,
            "rename",
            json!({"path": "src/util.rs", "line": 1, "column": 8, "new_name": "assist"}),
        )
        .await
        .unwrap();
        assert!(ok.contains("2 edit(s) in 2 file(s)"), "{ok}");
    }
}

/// A live check against an installed rust-analyzer, in the OS sandbox, in
/// hybrid mode: `cargo test -p declass-agent live_rust_analyzer -- --ignored
/// --nocapture`. The project is written to `DECLASS_LSP_LIVE_DIR` (default: a
/// temporary directory).
#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod live {
    use super::*;
    use crate::journal::WriteJournal;
    use crate::tools::{Outcome, dispatch};
    use declass_boundary::engine::Engine;
    use declass_boundary::policy::Policy;

    const EMAIL: &str = "amelia.velanwick42@mailbox-311.net";

    #[tokio::test]
    #[ignore = "needs rust-analyzer on PATH"]
    async fn live_rust_analyzer() {
        let tmp = tempfile::tempdir().unwrap();
        let base = std::env::var_os("DECLASS_LSP_LIVE_DIR")
            .map_or_else(|| tmp.path().to_path_buf(), PathBuf::from);
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let root = base.canonicalize().unwrap();
        let (ws, run) = (root.join("ws"), root.join("run"));
        for dir in ["src", "data"] {
            std::fs::create_dir_all(ws.join(dir)).unwrap();
        }
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(ws.join("Cargo.toml"), "[package]\nname = \"live\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n").unwrap();
        std::fs::write(ws.join("src/main.rs"), "mod util;\n\nfn main() {\n    let s = \"é😀\"; println!(\"{s} {}\", util::helper(2));\n}\n").unwrap();
        std::fs::write(
            ws.join("src/util.rs"),
            "/// Doubles a number.\npub fn helper(x: u32) -> u32 {\n    x * 2\n}\n",
        )
        .unwrap();
        std::fs::write(
            ws.join("data/owner.rs"),
            format!("// {EMAIL}\npub fn owner() {{}}\n"),
        )
        .unwrap();
        let policy = Policy {
            sensitive_globs: vec!["data/**".into()],
            detect_pii: true,
            ..Policy::default()
        };
        let engine = Engine::open(&run, policy, None).unwrap();
        engine.prime(
            &ws,
            &[
                "data/owner.rs".into(),
                "src/main.rs".into(),
                "src/util.rs".into(),
            ],
            "",
        );
        let settings = declass_lsp::Settings {
            enabled: true,
            request_timeout: Duration::from_secs(60),
            // The default: a check that takes longer is reported by a later edit.
            diagnostics_wait: Duration::from_secs(2),
            ..declass_lsp::Settings::default()
        };
        let sandbox = declass_sandbox::detect().unwrap();
        let lsp = language_servers(&settings, &ws, &run, sandbox).expect("rust-analyzer on PATH");
        let git = declass_git::Git::locate().unwrap();
        let mut journal = WriteJournal::open(&run).unwrap();
        let mut ctx = Ctx {
            workspace: &ws,
            run_dir: &run,
            sandbox,
            git: &git,
            presenter: engine.as_ref(),
            journal: &mut journal,
            command_timeout: Duration::from_secs(60),
            network: &crate::egress::Network::Off,
            checks: &[],
            audit: None,
            interrupted: None,
            web: None,
            git_tools: None,
            lsp: Some(&lsp),
        };
        let mut call = async |name: &str, args: Value| {
            let Value::Object(args) = args else {
                unreachable!()
            };
            match dispatch(&mut ctx, name, &args).await {
                Outcome::Result(s) => Ok(s),
                Outcome::Error(e) => Err(e),
                other => Err(format!("{other:?}")),
            }
        };
        // `helper` on line 4 is the 44th character, after a two-unit emoji.
        let nav = |op: &str| json!({"op": op, "path": "src/main.rs", "line": 4, "column": 44});
        let started = std::time::Instant::now();
        let def = loop {
            // The server answers before it has indexed the project.
            let r = call("code_nav", nav("definition")).await;
            if matches!(&r, Ok(t) if t.contains("src/util.rs"))
                || started.elapsed() > Duration::from_secs(120)
            {
                break r;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        };
        println!(
            "definition ({:.1}s):\n{def:?}",
            started.elapsed().as_secs_f64()
        );
        assert!(
            def.unwrap()
                .contains("src/util.rs:2:8  pub fn helper(x: u32) -> u32 {")
        );
        let refs = call("code_nav", nav("references")).await.unwrap();
        println!("references:\n{refs}");
        assert!(refs.contains("src/main.rs:4:44"), "{refs}");
        let hover = call("code_nav", nav("hover")).await.unwrap();
        println!("hover:\n{hover}");
        assert!(hover.contains("Doubles a number."), "{hover}");
        let symbols = call("code_nav", json!({"op": "symbols", "path": "src/util.rs"}))
            .await
            .unwrap();
        println!("symbols:\n{symbols}");
        let ws_symbols = call(
            "code_nav",
            json!({"op": "workspace_symbols", "query": "helper"}),
        )
        .await
        .unwrap();
        println!("workspace symbols:\n{ws_symbols}");
        let sensitive = call(
            "code_nav",
            json!({"op": "symbols", "path": "data/owner.rs"}),
        )
        .await
        .unwrap_err();
        println!("sensitive: {sensitive}");
        let t = std::time::Instant::now();
        let edited = call(
            "edit_file",
            json!({"path": "src/util.rs", "edits": [{"old": "x * 2", "new": "x * \"2\""}]}),
        )
        .await
        .unwrap();
        println!("edit ({:.1}s):\n{edited}", t.elapsed().as_secs_f64());
        let t = std::time::Instant::now();
        let renamed = call(
            "rename",
            json!({"path": "src/util.rs", "line": 2, "column": 8, "new_name": "double"}),
        )
        .await
        .unwrap();
        println!("rename ({:.1}s):\n{renamed}", t.elapsed().as_secs_f64());
        assert!(
            std::fs::read_to_string(ws.join("src/main.rs"))
                .unwrap()
                .contains("util::double(2)")
        );
        let diags = call(
            "code_nav",
            json!({"op": "diagnostics", "path": "src/util.rs"}),
        )
        .await
        .unwrap();
        println!("diagnostics:\n{diags}");
        for text in [
            &refs,
            &hover,
            &symbols,
            &ws_symbols,
            &edited,
            &renamed,
            &diags,
        ] {
            assert!(!text.contains(EMAIL));
        }
        lsp.shutdown().await;
    }
}
