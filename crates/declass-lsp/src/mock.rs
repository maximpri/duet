// SPDX-License-Identifier: GPL-3.0-or-later
//! A scripted language server for tests: in-process ([`MockLauncher`]) or as
//! the `declass-lsp-mock` program.
//!
//! Its "language" is tiny: `fn NAME` defines NAME, every other occurrence of
//! the word refers to it, and a `///` line above a definition documents it.
//! It reads the files under the root from disk (skipping `.git`, `.declass` and
//! `target`) plus the open documents, publishes a diagnostic for every line
//! containing `ERROR` (error) or `WARN` (warning), and asks the client the
//! three requests servers commonly send (`workspace/configuration`,
//! `window/workDoneProgress/create`, `client/registerCapability`). Hovering
//! a word `crash` makes it exit; `crash_once` only in its first session.
//! With `slow_check`, a save publishes its diagnostics late, inside a
//! work-done progress.
//! Positions are computed in UTF-16 here, independently of the client.

use crate::client::Transport;
use crate::framing::{read_message, write_message};
use crate::manager::{Launcher, Launching};
use crate::servers::Detected;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncRead, AsyncWrite, BufReader};
use url::Url;

#[derive(Debug, Clone, Default)]
pub struct MockOptions {
    /// Extensions of the files it reads from disk (default `rs`).
    pub extensions: Vec<String>,
    /// Rename answers use `documentChanges` (with versions) instead of `changes`.
    pub document_changes: bool,
    /// Rename answers also create a file (a resource operation).
    pub resource_op: bool,
    /// Requests of this method are never answered.
    pub hang_on: Option<String>,
    /// A save first publishes no diagnostics inside a work-done progress,
    /// then, 400 ms later, the real ones and the end of the progress (as a
    /// server that runs a slower check on save does).
    pub slow_check: bool,
}

impl MockOptions {
    /// From program arguments: `--document-changes`, `--resource-op`,
    /// `--hang-on <method>`, `--ext <ext>`.
    pub fn from_args(args: &[String]) -> Self {
        let mut o = Self::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--document-changes" => o.document_changes = true,
                "--resource-op" => o.resource_op = true,
                "--hang-on" => o.hang_on = it.next().cloned(),
                "--slow-check" => o.slow_check = true,
                "--ext" => o.extensions.extend(it.next().cloned()),
                _ => {}
            }
        }
        o
    }
}

/// How a session ended.
#[derive(Debug, PartialEq, Eq)]
pub enum End {
    Exit,
    Crash,
    Eof,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn utf16_len(s: &str) -> u32 {
    s.chars().map(|c| c.len_utf16() as u32).sum()
}

/// Every whole-word occurrence of `word` in `line`: (start, end) in UTF-16 units.
fn occurrences(line: &str, word: &str) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for (i, _) in line.match_indices(word) {
        let before = line[..i].chars().next_back();
        let after = line[i + word.len()..].chars().next();
        if before.is_none_or(|c| !is_word(c)) && after.is_none_or(|c| !is_word(c)) {
            let start = utf16_len(&line[..i]);
            out.push((start, start + utf16_len(word)));
        }
    }
    out
}

/// The word at a UTF-16 column of `line`.
fn word_at(line: &str, character: u32) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut units = 0;
    let mut idx = None;
    for (i, c) in chars.iter().enumerate() {
        let next = units + c.len_utf16() as u32;
        if character < next {
            idx = Some(i);
            break;
        }
        units = next;
    }
    let i = idx?;
    if !is_word(chars[i]) {
        return None;
    }
    let start = (0..=i).rev().take_while(|&j| is_word(chars[j])).last()?;
    let end = (i..chars.len()).take_while(|&j| is_word(chars[j])).last()?;
    Some(chars[start..=end].iter().collect())
}

fn range(line: usize, (s, e): (u32, u32)) -> Value {
    json!({"start": {"line": line, "character": s}, "end": {"line": line, "character": e}})
}

struct State {
    root: PathBuf,
    session: usize,
    opts: MockOptions,
    open: BTreeMap<PathBuf, (i64, String)>,
    log: Vec<Value>,
}

impl State {
    /// Every file of the language: open documents over what is on disk.
    fn files(&self) -> BTreeMap<PathBuf, String> {
        let exts: Vec<&str> = if self.opts.extensions.is_empty() {
            vec!["rs"]
        } else {
            self.opts.extensions.iter().map(String::as_str).collect()
        };
        let mut out = BTreeMap::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if p.is_dir() {
                    if ![".git", ".declass", "target"].contains(&name.as_str()) {
                        stack.push(p);
                    }
                } else if p
                    .extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| exts.contains(&x))
                    && let Ok(text) = std::fs::read_to_string(&p)
                {
                    out.insert(p, text);
                }
            }
        }
        for (p, (_, text)) in &self.open {
            out.insert(p.clone(), text.clone());
        }
        out
    }

    fn text_of(&self, path: &Path) -> Option<String> {
        self.open
            .get(path)
            .map(|(_, t)| t.clone())
            .or_else(|| std::fs::read_to_string(path).ok())
    }

    /// (file, line, columns) of every `fn word`.
    fn definitions(&self, word: &str) -> Vec<(PathBuf, usize, (u32, u32), String)> {
        let mut out = Vec::new();
        for (p, text) in self.files() {
            for (n, line) in text.lines().enumerate() {
                for (s, e) in occurrences(line, word) {
                    let before: String = line.chars().take(s as usize).collect();
                    if before.trim_end().ends_with("fn") {
                        out.push((p.clone(), n, (s, e), line.to_owned()));
                    }
                }
            }
        }
        out
    }

    fn word(&self, params: &Value) -> Option<(PathBuf, String)> {
        let uri = params.pointer("/textDocument/uri")?.as_str()?;
        let path = Url::parse(uri).ok()?.to_file_path().ok()?;
        let line = params.pointer("/position/line")?.as_u64()? as usize;
        let ch = params.pointer("/position/character")?.as_u64()? as u32;
        let text = self.text_of(&path)?;
        let l = text.lines().nth(line)?;
        Some((path, word_at(l, ch)?))
    }

    fn location(p: &Path, line: usize, cols: (u32, u32)) -> Value {
        let uri = Url::from_file_path(p)
            .map(|u| u.to_string())
            .unwrap_or_default();
        json!({"uri": uri, "range": range(line, cols)})
    }

    fn diagnostics(&self, path: &Path) -> Value {
        let (version, text) = self.open.get(path).cloned().unwrap_or_default();
        let mut items = Vec::new();
        for (n, line) in text.lines().enumerate() {
            for (marker, severity) in [("ERROR", 1), ("WARN", 2)] {
                if let Some(i) = line.find(marker) {
                    let s = utf16_len(&line[..i]);
                    items.push(json!({
                        "range": range(n, (s, s + marker.len() as u32)),
                        "severity": severity,
                        "source": "mock",
                        "message": format!("{marker} marker here\nsecond line of the message"),
                    }));
                }
            }
        }
        let uri = Url::from_file_path(path)
            .map(|u| u.to_string())
            .unwrap_or_default();
        json!({"uri": uri, "version": version, "diagnostics": items})
    }

    fn handle(&mut self, method: &str, params: &Value) -> Result<Value, End> {
        Ok(match method {
            "initialize" => {
                if let Some(root) = params
                    .get("rootUri")
                    .and_then(Value::as_str)
                    .and_then(|u| Url::parse(u).ok())
                    .and_then(|u| u.to_file_path().ok())
                {
                    self.root = root;
                }
                self.log
                    .push(json!({"initialize": params.get("capabilities").cloned()}));
                json!({
                    "capabilities": {
                        "positionEncoding": "utf-16",
                        "textDocumentSync": {"openClose": true, "change": 1, "save": {"includeText": false}},
                        "definitionProvider": true,
                        "referencesProvider": true,
                        "hoverProvider": true,
                        "documentSymbolProvider": true,
                        "workspaceSymbolProvider": true,
                        "renameProvider": true
                    },
                    "serverInfo": {"name": "declass-lsp-mock"}
                })
            }
            "shutdown" => Value::Null,
            "mock/log" => Value::Array(self.log.clone()),
            "textDocument/definition" => {
                let Some((_, word)) = self.word(params) else {
                    return Ok(Value::Null);
                };
                Value::Array(
                    self.definitions(&word)
                        .into_iter()
                        .map(|(p, n, c, _)| Self::location(&p, n, c))
                        .collect(),
                )
            }
            "textDocument/references" => {
                let Some((_, word)) = self.word(params) else {
                    return Ok(Value::Null);
                };
                let mut out = Vec::new();
                for (p, text) in self.files() {
                    for (n, line) in text.lines().enumerate() {
                        for c in occurrences(line, &word) {
                            out.push(Self::location(&p, n, c));
                        }
                    }
                }
                Value::Array(out)
            }
            "textDocument/hover" => {
                let Some((_, word)) = self.word(params) else {
                    return Ok(Value::Null);
                };
                if word == "crash" || (word == "crash_once" && self.session == 1) {
                    return Err(End::Crash);
                }
                let Some((p, n, _, line)) = self.definitions(&word).into_iter().next() else {
                    return Ok(Value::Null);
                };
                let doc = self
                    .text_of(&p)
                    .and_then(|t| {
                        n.checked_sub(1)
                            .and_then(|m| t.lines().nth(m).map(str::to_owned))
                    })
                    .filter(|l| l.trim_start().starts_with("///"))
                    .map(|l| l.trim_start().trim_start_matches('/').trim().to_owned())
                    .unwrap_or_default();
                json!({"contents": {"kind": "markdown",
                    "value": format!("```\n{}\n```\n{doc}", line.trim())}})
            }
            "textDocument/documentSymbol" => {
                let Some(path) = params
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .and_then(|u| Url::parse(u).ok())
                    .and_then(|u| u.to_file_path().ok())
                else {
                    return Ok(Value::Null);
                };
                let text = self.text_of(&path).unwrap_or_default();
                let mut out = Vec::new();
                for (n, line) in text.lines().enumerate() {
                    if let Some(i) = line.find("fn ") {
                        let name: String =
                            line[i + 3..].chars().take_while(|c| is_word(*c)).collect();
                        let s = utf16_len(&line[..i + 3]);
                        let r = range(n, (s, s + utf16_len(&name)));
                        out.push(json!({"name": name, "kind": 12, "detail": line.trim(),
                            "range": r, "selectionRange": r, "children": []}));
                    }
                }
                Value::Array(out)
            }
            "workspace/symbol" => {
                let q = params.get("query").and_then(Value::as_str).unwrap_or("");
                let mut out = Vec::new();
                for (p, text) in self.files() {
                    for (n, line) in text.lines().enumerate() {
                        if let Some(i) = line.find("fn ") {
                            let name: String =
                                line[i + 3..].chars().take_while(|c| is_word(*c)).collect();
                            if name.contains(q) {
                                let s = utf16_len(&line[..i + 3]);
                                out.push(json!({"name": name, "kind": 12,
                                    "location": Self::location(&p, n, (s, s + utf16_len(&name)))}));
                            }
                        }
                    }
                }
                Value::Array(out)
            }
            "textDocument/rename" => {
                let Some((_, word)) = self.word(params) else {
                    return Ok(Value::Null);
                };
                let new = params.get("newName").and_then(Value::as_str).unwrap_or("");
                let mut by_file: BTreeMap<String, Vec<Value>> = BTreeMap::new();
                let mut versions = BTreeMap::new();
                for (p, text) in self.files() {
                    let uri = Url::from_file_path(&p)
                        .map(|u| u.to_string())
                        .unwrap_or_default();
                    for (n, line) in text.lines().enumerate() {
                        for c in occurrences(line, &word) {
                            by_file
                                .entry(uri.clone())
                                .or_default()
                                .push(json!({"range": range(n, c), "newText": new}));
                        }
                    }
                    versions.insert(uri, self.open.get(&p).map(|(v, _)| *v));
                }
                if self.opts.document_changes || self.opts.resource_op {
                    let mut changes: Vec<Value> = by_file
                        .into_iter()
                        .map(|(uri, edits)| {
                            json!({"textDocument": {"uri": uri, "version": versions.get(&uri).cloned().flatten()},
                                   "edits": edits})
                        })
                        .collect();
                    if self.opts.resource_op {
                        let uri = Url::from_file_path(self.root.join("new.rs"))
                            .map(|u| u.to_string())
                            .unwrap_or_default();
                        changes.push(json!({"kind": "create", "uri": uri}));
                    }
                    json!({"documentChanges": changes})
                } else {
                    json!({"changes": by_file})
                }
            }
            _ => Value::Null,
        })
    }
}

/// Serves one session (numbered from 1) on `read`/`write` until `exit`, a
/// crash or the end of input.
pub async fn serve<R, W>(read: R, mut write: W, opts: MockOptions, session: usize) -> End
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = BufReader::new(read);
    let mut st = State {
        root: PathBuf::from("/"),
        session,
        opts,
        open: BTreeMap::new(),
        log: Vec::new(),
    };
    let mut next_id = 1000;
    let mut asked: BTreeMap<i64, String> = BTreeMap::new();
    loop {
        let msg = match read_message(&mut reader).await {
            Ok(Some(m)) => m,
            _ => return End::Eof,
        };
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let mut out = Vec::new();
        match msg.get("id").cloned() {
            // A reply to one of our requests.
            Some(id) if method.is_empty() => {
                let asked_for = id
                    .as_i64()
                    .and_then(|i| asked.remove(&i))
                    .unwrap_or_default();
                st.log.push(json!({"reply_to": asked_for, "result": msg.get("result"), "error": msg.get("error")}));
            }
            Some(id) => {
                if st.opts.hang_on.as_deref() == Some(method.as_str()) {
                    continue;
                }
                match st.handle(&method, &params) {
                    Ok(result) => out.push(json!({"jsonrpc": "2.0", "id": id, "result": result})),
                    Err(end) => return end,
                }
            }
            None => match method.as_str() {
                "initialized" => {
                    for (m, p) in [
                        (
                            "workspace/configuration",
                            json!({"items": [{"section": "mock"}]}),
                        ),
                        ("window/workDoneProgress/create", json!({"token": "index"})),
                        ("client/registerCapability", json!({"registrations": []})),
                    ] {
                        next_id += 1;
                        asked.insert(next_id, m.to_owned());
                        out.push(
                            json!({"jsonrpc": "2.0", "id": next_id, "method": m, "params": p}),
                        );
                    }
                }
                "exit" => return End::Exit,
                "textDocument/didOpen" | "textDocument/didChange" | "textDocument/didSave" => {
                    st.log.push(json!({"notification": method}));
                    let Some(path) = params
                        .pointer("/textDocument/uri")
                        .and_then(Value::as_str)
                        .and_then(|u| Url::parse(u).ok())
                        .and_then(|u| u.to_file_path().ok())
                    else {
                        continue;
                    };
                    let version = params
                        .pointer("/textDocument/version")
                        .and_then(Value::as_i64);
                    if let Some(text) = params.pointer("/textDocument/text").and_then(Value::as_str)
                    {
                        st.open
                            .insert(path.clone(), (version.unwrap_or(1), text.to_owned()));
                    }
                    if let Some(text) = params
                        .pointer("/contentChanges/0/text")
                        .and_then(Value::as_str)
                    {
                        st.open
                            .insert(path.clone(), (version.unwrap_or(1), text.to_owned()));
                    }
                    if method == "textDocument/didSave" && st.opts.slow_check {
                        let mut quick = st.diagnostics(&path);
                        quick["diagnostics"] = json!([]);
                        let progress = |kind: &str| {
                            json!({"jsonrpc": "2.0", "method": "$/progress",
                                "params": {"token": "check", "value": {"kind": kind, "title": "check"}}})
                        };
                        for m in [
                            progress("begin"),
                            json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": quick}),
                        ] {
                            if write_message(&mut write, &m).await.is_err() {
                                return End::Eof;
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                        out.push(
                            json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                            "params": st.diagnostics(&path)}),
                        );
                        out.push(progress("end"));
                    } else if method != "textDocument/didChange" {
                        out.push(
                            json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                            "params": st.diagnostics(&path)}),
                        );
                    }
                }
                _ => {}
            },
        }
        for m in out {
            if write_message(&mut write, &m).await.is_err() {
                return End::Eof;
            }
        }
    }
}

/// Runs the mock in-process: every launch is a fresh session over an
/// in-memory pipe. `launches` counts them; a crash ends the session and
/// closes the pipe, as a dying process would.
#[derive(Clone, Default)]
pub struct MockLauncher {
    pub opts: MockOptions,
    pub launches: Arc<AtomicUsize>,
    /// The hidden paths each launch was given.
    pub denied: Arc<std::sync::Mutex<Vec<Vec<PathBuf>>>>,
}

impl Launcher for MockLauncher {
    fn launch<'a>(&'a self, _server: &'a Detected, deny_read: &'a [PathBuf]) -> Launching<'a> {
        let session = self.launches.fetch_add(1, Ordering::SeqCst) + 1;
        self.denied
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(deny_read.to_vec());
        let (client, server) = tokio::io::duplex(1 << 20);
        let (sr, sw) = tokio::io::split(server);
        let (cr, cw) = tokio::io::split(client);
        let opts = self.opts.clone();
        tokio::spawn(async move {
            serve(sr, sw, opts, session).await;
        });
        Box::pin(std::future::ready(Ok(Transport {
            read: Box::new(cr),
            write: Box::new(cw),
            stderr: None,
            process: None,
        })))
    }
}

/// A server entry that the mock stands in for (Rust files).
pub fn detected(language: &str, extensions: &[&str]) -> Detected {
    Detected {
        spec: crate::servers::ServerSpec {
            language: language.into(),
            candidates: vec![("declass-lsp-mock".into(), vec![])],
            extensions: extensions.iter().map(|e| (*e).into()).collect(),
            env: Vec::new(),
            init_options: Value::Null,
            settings: json!({"mock": {"answer": 42}}),
            configured: true,
        },
        program: PathBuf::from("declass-lsp-mock"),
        args: Vec::new(),
    }
}
