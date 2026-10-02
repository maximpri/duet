// SPDX-License-Identifier: GPL-3.0-or-later
//! Protected source (IP levels) in the engine.
//!
//! - Interface-only files are shown as a skeleton; every withheld body or
//!   private value is a handle the local model can answer questions about.
//! - Sealed files (and interface-only files without a skeleton) are shown as
//!   a notice with one handle for the whole file.
//! - Ordinary commands cannot read protected files. Checks, and commands run
//!   with `sensitive_data`, can; from their output only recognised result lines
//!   are shown, and every line quoting protected code is withheld.
//! - Protected files change only through the local model (`edit_protected`).
//! - Withheld text is indexed three ways: copied spans (overlap filter), whole
//!   distinctive lines, and distinctive literals (placeholder vault).

use super::{COMMAND_SCAN_SKIP, Engine, PRIME_MAX_BYTES, State, safe_local_error};
use crate::detect::Kind;
use crate::ip::{ProtectedLines, distinctive_literals, result_lines};
use crate::model::ToolSpec;
use crate::policy::IpLevel;
use crate::skeleton::{Lang, Span, SpanKind, render, withheld};
use crate::view::{ImplementRequest, Implemented, Source, ViewClass};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Protected-source state, kept with the rest of the engine state.
#[derive(Default)]
pub(super) struct IpState {
    pub(super) lines: ProtectedLines,
    /// Stable handles: sha256(path, withheld text) → handle id.
    handles: BTreeMap<String, String>,
    handles_file: Option<PathBuf>,
    workspace: Option<PathBuf>,
    /// Protected files found when priming, for the note added to the task.
    files: Vec<(String, IpLevel)>,
}

impl IpState {
    pub(super) fn open(run_dir: &Path) -> Self {
        let file = run_dir.join("ip-handles.json");
        Self {
            handles: std::fs::read(&file)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default(),
            handles_file: Some(file),
            ..Self::default()
        }
    }
}

fn level_name(level: IpLevel) -> &'static str {
    match level {
        IpLevel::InterfaceOnly => "interface-only",
        IpLevel::Sealed => "sealed",
    }
}

/// Text read with line numbers (`read_file`), without them.
fn unnumbered(text: &str) -> String {
    text.lines()
        .map(|l| {
            let t = l.trim_start();
            let digits = t.len() - t.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            if digits > 0 && t[digits..].starts_with("  ") {
                &t[digits + 2..]
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The skeleton of `text`, or why there is none.
fn spans_of(path: &Path, text: &str) -> Result<Vec<Span>, String> {
    let lang = Lang::for_path(path).ok_or("no interface view for this file type")?;
    withheld(lang, text)
}

impl Engine {
    fn ip_level(&self, path: &Path) -> Option<IpLevel> {
        self.policy.ip_level(path)
    }

    /// `files` without the protected ones (which [`Engine::ip_prime`] indexes).
    pub(super) fn ip_split(&self, files: &[String]) -> Vec<String> {
        files
            .iter()
            .filter(|f| self.ip_level(Path::new(f)).is_none())
            .cloned()
            .collect()
    }

    /// Indexes every protected file, so copies of withheld code are caught
    /// wherever they appear. Runs after the public files were read.
    pub(super) fn ip_prime(&self, workspace: &Path, files: &[String]) -> usize {
        let mut st = self.lock();
        st.ip.workspace = Some(workspace.to_path_buf());
        let mut n = 0;
        for f in files {
            let path = Path::new(f);
            let Some(level) = self.ip_level(path) else {
                continue;
            };
            st.ip.files.push((f.clone(), level));
            if let Ok(bytes) = duet_fs::read_file(workspace, path, PRIME_MAX_BYTES as u64) {
                self.ip_index(&mut st, path, level, &String::from_utf8_lossy(&bytes));
                n += 1;
            }
        }
        n
    }

    /// Indexes withheld text; returns the skeleton spans if the file has a skeleton.
    pub(super) fn ip_index(
        &self,
        st: &mut State,
        path: &Path,
        level: IpLevel,
        text: &str,
    ) -> Option<Vec<Span>> {
        let spans = match level {
            IpLevel::InterfaceOnly => spans_of(path, text).ok(),
            IpLevel::Sealed => None,
        };
        let hidden: Vec<&str> = match &spans {
            Some(spans) => {
                let skeleton = render(text, spans, |_| String::new());
                st.ip.lines.add_public(&skeleton);
                st.overlap.add_public(&skeleton);
                spans.iter().map(|s| &text[s.start..s.end]).collect()
            }
            None => vec![text],
        };
        let origin = path.display().to_string();
        for h in hidden {
            st.ip.lines.add_protected(h);
            st.overlap.add_sensitive(h);
            for lit in distinctive_literals(h, &st.public_words) {
                let _ = st
                    .vault
                    .token_for(&lit, Kind::Code, Some("literal"), &origin);
            }
        }
        spans
    }

    /// The handle holding `text` (the same text always gets the same handle).
    fn ip_handle(&self, st: &mut State, path: &Path, what: &str, text: &str) -> String {
        let key = hex::encode(Sha256::digest(
            format!("{}\0{text}", path.display()).as_bytes(),
        ));
        if let Some(id) = st.ip.handles.get(&key)
            && st.handles.get(id).is_some()
        {
            return id.clone();
        }
        let label = format!("{} ({what})", path.display());
        match st.handles.put(text.as_bytes(), &label) {
            Ok(h) => {
                st.ip.handles.insert(key, h.id.clone());
                if let Some(f) = &st.ip.handles_file {
                    let _ = duet_fs::private::write_private(
                        f,
                        &serde_json::to_vec(&st.ip.handles).unwrap_or_default(),
                    );
                }
                h.id
            }
            Err(_) => "unavailable".into(),
        }
    }

    /// The whole file's text: read again from the workspace (so line ranges do
    /// not cut a skeleton), or recovered from what `read_file` passed.
    pub(super) fn ip_full_text(&self, st: &State, path: &Path, shown: &str) -> String {
        st.ip
            .workspace
            .as_ref()
            .and_then(|ws| duet_fs::read_file(ws, path, PRIME_MAX_BYTES as u64).ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_else(|| unnumbered(shown))
    }

    fn ip_file_view(&self, path: &Path, level: IpLevel, shown: &str) -> String {
        let mut st = self.lock();
        let text = self.ip_full_text(&st, path, shown);
        let spans = self.ip_index(&mut st, path, level, &text);
        let p = path.display();
        match spans {
            Some(spans) => {
                let skeleton = render(&text, &spans, |s| {
                    let body = &text[s.start..s.end];
                    match s.kind {
                        SpanKind::Value => {
                            let what = format!("value of {}", s.item);
                            format!("⟨value:{}⟩", self.ip_handle(&mut st, path, &what, body))
                        }
                        _ => {
                            let what = format!("body of {}", s.item);
                            format!("⟨body:{}⟩", self.ip_handle(&mut st, path, &what, body))
                        }
                    }
                });
                let skeleton = self.sanitize(&mut st, &skeleton, &p.to_string(), false);
                format!(
                    "[{p} is interface-only: the whole file is shown as signatures, types, doc comments and public \
constants (line numbers and ranges do not apply). Each ⟨body:hN⟩ or ⟨value:hN⟩ is a handle: ask_local(handle=\"hN\", \
...) asks the local model about it. Change this file with edit_protected.]\n{skeleton}"
                )
            }
            None => {
                let id = self.ip_handle(&mut st, path, "whole file", &text);
                let why = match level {
                    IpLevel::Sealed => String::new(),
                    IpLevel::InterfaceOnly => format!(
                        " (no interface view: {})",
                        spans_of(path, &text).err().unwrap_or_default()
                    ),
                };
                format!(
                    "[{p} is sealed{why}: its content is never shown. {id} holds it: ask_local(handle=\"{id}\", ...) \
asks the local model about it. Change it with edit_protected.]\n"
                )
            }
        }
    }

    /// Output of a run that could read protected source: a handle, and only
    /// the recognised result lines, sanitized.
    fn ip_run_view(&self, label: &str, text: &str, sensitive: bool) -> String {
        let mut st = self.lock();
        if sensitive {
            st.overlap.add_sensitive(text);
        }
        let handle = match st.handles.put(text.as_bytes(), label) {
            Ok(h) => h.id,
            Err(e) => return format!("[output withheld: could not store it locally: {e}]"),
        };
        let (kept, withheld) = result_lines(text);
        let mut out = format!(
            "{handle} ({label}): {} lines. This run could read protected source, so only recognised result lines \
are shown ({withheld} other line(s) withheld); ask_local(handle=\"{handle}\", ...) about the rest.\n",
            text.lines().count()
        );
        for line in kept {
            let clean = self.sanitize(&mut st, &line, label, true);
            out.push_str(&st.ip.lines.redact(&clean).0);
            out.push('\n');
        }
        out
    }

    fn ip_marker(&self, path: &str) -> Option<String> {
        self.ip_level(Path::new(path.trim()))
            .map(|l| format!("{}  [{}]", path.trim_end(), level_name(l)))
    }

    /// What the frontier sees of protected content; `None` leaves the result
    /// to the rest of the engine.
    pub(super) fn ip_present(&self, source: &Source, text: &str) -> Option<String> {
        if !self.policy.has_ip() {
            return None;
        }
        match source {
            Source::File { path, .. } => {
                let level = self.ip_level(path)?;
                self.set_class(ViewClass::Protected);
                Some(self.ip_file_view(path, level, text))
            }
            Source::Diff => Some(self.ip_diff(text)),
            Source::SensitiveCommand { command, .. } => {
                self.set_class(ViewClass::HandleSummary);
                Some(self.ip_run_view(
                    &format!("output of `{command}` (ran with sensitive data)"),
                    text,
                    true,
                ))
            }
            Source::Checks => {
                self.set_class(ViewClass::HandleSummary);
                Some(self.ip_run_view("check output", text, false))
            }
            _ => None,
        }
    }

    /// Listings and search results with protected paths marked and their
    /// matching lines removed, for the rest of the engine to present.
    pub(super) fn ip_prefilter(&self, source: &Source, text: &str) -> Option<String> {
        if !self.policy.has_ip() {
            return None;
        }
        match source {
            Source::FileList => Some(
                text.lines()
                    .map(|l| self.ip_marker(l).unwrap_or_else(|| l.to_owned()) + "\n")
                    .collect(),
            ),
            Source::Search { .. } => Some(
                text.lines()
                    .map(|line| {
                        let path = line.split(':').next().unwrap_or_default();
                        if !path.is_empty() && self.ip_level(Path::new(path)).is_some() {
                            let loc: String =
                                line.splitn(3, ':').take(2).collect::<Vec<_>>().join(":");
                            format!("{loc}: [match in protected source]\n")
                        } else {
                            format!("{line}\n")
                        }
                    })
                    .collect(),
            ),
            _ => None,
        }
    }

    /// A diff with the sections of protected files replaced by a note.
    fn ip_diff(&self, text: &str) -> String {
        let mut out = String::new();
        let mut section = String::new();
        let flush = |section: &mut String, out: &mut String| {
            if section.is_empty() {
                return;
            }
            let path = section
                .lines()
                .next()
                .and_then(|h| h.strip_prefix("diff --git a/"))
                .and_then(|h| h.split(" b/").next())
                .map(str::to_owned);
            match path.filter(|p| self.ip_level(Path::new(p)).is_some()) {
                Some(p) => out.push_str(&format!(
                    "diff --git a/{p} b/{p}\n[changes to protected source are not shown]\n"
                )),
                None => {
                    let mut st = self.lock();
                    out.push_str(&self.sanitize(&mut st, section, "diff", false));
                }
            }
            section.clear();
        };
        for line in text.split_inclusive('\n') {
            if line.starts_with("diff --git ") {
                flush(&mut section, &mut out);
            }
            section.push_str(line);
        }
        flush(&mut section, &mut out);
        out
    }

    /// Protected files and directories (ordinary commands may not read them).
    pub(super) fn ip_hidden(&self, workspace: &Path) -> Vec<PathBuf> {
        if !self.policy.has_ip() {
            return Vec::new();
        }
        let mut out: Vec<PathBuf> = self
            .policy
            .interface_only
            .iter()
            .chain(&self.policy.sealed)
            .filter_map(|g| g.strip_suffix("/**"))
            .filter(|dir| !dir.contains(['*', '?', '[']))
            .map(|dir| workspace.join(dir))
            .filter(|p| p.is_dir())
            .collect();
        let mut stack = vec![PathBuf::new()];
        while let Some(rel) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(workspace.join(&rel)) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let child = rel.join(&name);
                let abs = workspace.join(&child);
                if out.iter().any(|d| abs.starts_with(d)) {
                    continue;
                }
                match entry.file_type() {
                    Ok(t) if t.is_dir() => {
                        if !COMMAND_SCAN_SKIP.contains(&name.to_string_lossy().as_ref()) {
                            stack.push(child);
                        }
                    }
                    Ok(t) if t.is_file() && self.ip_level(&child).is_some() => out.push(abs),
                    _ => {}
                }
            }
        }
        out
    }

    /// What checks may not read: everything hidden from commands except protected source.
    pub(super) fn ip_hidden_from_checks(
        &self,
        hidden: Vec<PathBuf>,
        workspace: &Path,
    ) -> Vec<PathBuf> {
        hidden
            .into_iter()
            .filter(|p| {
                p.strip_prefix(workspace)
                    .map_or(true, |rel| self.ip_level(rel).is_none())
            })
            .collect()
    }

    /// Protected paths may not be written with ordinary edits.
    pub(super) fn ip_guard_write(&self, path: &Path) -> Result<(), String> {
        match self.ip_level(path) {
            Some(level) => Err(format!(
                "{} is {} source; change it with edit_protected (the local model implements your spec)",
                path.display(),
                level_name(level)
            )),
            None => Ok(()),
        }
    }

    /// The note added to the task when some source is protected.
    pub(super) fn ip_note(st: &State) -> String {
        if st.ip.files.is_empty() {
            return String::new();
        }
        let list: Vec<String> = st
            .ip
            .files
            .iter()
            .take(super::LISTED_PATHS)
            .map(|(p, l)| format!("{p} ({})", level_name(*l)))
            .collect();
        format!(
            "\n\nProtected source (you see signatures only, or nothing if sealed): {}. Commands cannot read these \
files: to build or test code that includes them use run_command with sensitive_data (only recognised result lines come \
back), and change them with edit_protected.",
            list.join(", ")
        )
    }

    pub(super) fn ip_tools(&self) -> Vec<ToolSpec> {
        if !self.policy.has_ip() {
            return Vec::new();
        }
        vec![ToolSpec {
            name: "edit_protected".into(),
            description: "Change a protected (interface-only or sealed) file. You cannot see its implementation, so \
describe the change in `spec`: the behaviour wanted, inputs and outputs, edge cases, and which functions or types it \
concerns. The local model implements it on this machine; the host writes the file and runs `command` (default: the \
configured checks) with the protected source readable. You get pass/fail, the recognised result lines and which items \
changed. `tests` (optional) is test code written to an open file first and shown to the local model. If the checks \
fail, the local model gets their output and one more attempt; the last version is kept either way."
                .into(),
            parameters: json!({"type": "object", "properties": {
                "path": {"type": "string"},
                "spec": {"type": "string", "description": "What to change, precisely."},
                "tests": {"type": "object", "properties": {
                    "path": {"type": "string", "description": "An open (unprotected) test file."},
                    "content": {"type": "string"}
                }, "required": ["path", "content"]},
                "command": {"type": "string", "description": "Command that verifies the change, e.g. a test run."}
            }, "required": ["path", "spec"]}),
        }]
    }

    /// Lines of outbound text that quote protected code, withheld.
    pub(super) fn ip_redact(st: &mut State, text: &str) -> (String, usize) {
        if st.ip.lines.is_empty() {
            return (text.to_owned(), 0);
        }
        st.ip.lines.redact(text)
    }

    pub(super) fn ip_implement(
        &self,
        path: &Path,
        current: &str,
        req: &ImplementRequest<'_>,
    ) -> Option<Result<Implemented, String>> {
        let level = self.ip_level(path)?;
        Some(self.ip_implement_level(path, level, current, req))
    }

    fn ip_implement_level(
        &self,
        path: &Path,
        level: IpLevel,
        current: &str,
        req: &ImplementRequest<'_>,
    ) -> Result<Implemented, String> {
        let local = self.local.as_ref().ok_or("no local model is configured")?;
        let (spec, tests) = {
            let st = self.lock();
            (
                st.vault.detokenize(req.spec).0,
                req.tests.map(|t| st.vault.detokenize(t).0),
            )
        };
        let label = path.display().to_string();
        let code =
            Self::block_on(local.implement(&label, current, &spec, tests.as_deref(), req.feedback))
                .map_err(|e| format!("local model: {}", safe_local_error(&e)))?;
        let before = match level {
            IpLevel::InterfaceOnly => spans_of(path, current).ok(),
            IpLevel::Sealed => None,
        };
        let after = match (&before, level) {
            (Some(_), _) => Some(spans_of(path, &code).map_err(|e| {
                format!("the local model's version of {label} was not written: {e}")
            })?),
            _ => None,
        };
        let summary = match (before, after) {
            (Some(b), Some(a)) => change_summary(current, &b, &code, &a),
            _ => format!("{label} ({}) was rewritten", level_name(level)),
        };
        let mut st = self.lock();
        self.ip_index(&mut st, path, level, &code);
        Ok(Implemented {
            content: code,
            summary,
        })
    }
}

/// Which items changed between two versions, described by the interface only.
fn change_summary(old: &str, old_spans: &[Span], new: &str, new_spans: &[Span]) -> String {
    let items = |text: &str, spans: &[Span]| -> BTreeMap<String, Vec<String>> {
        let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for s in spans.iter().filter(|s| s.kind != SpanKind::Comment) {
            m.entry(s.item.clone())
                .or_default()
                .push(text[s.start..s.end].to_owned());
        }
        m
    };
    let (a, b) = (items(old, old_spans), items(new, new_spans));
    let mut changed = Vec::new();
    let mut added = Vec::new();
    let mut removed = Vec::new();
    for (k, v) in &b {
        match a.get(k) {
            None => added.push(k.clone()),
            Some(o) if o != v => changed.push(k.clone()),
            _ => {}
        }
    }
    for k in a.keys() {
        if !b.contains_key(k) {
            removed.push(k.clone());
        }
    }
    let neutral = |text: &str, spans: &[Span]| render(text, spans, |_| String::new());
    let interface = if neutral(old, old_spans) == neutral(new, new_spans) {
        "the interface (signatures, types, doc comments, public constants) is unchanged"
    } else {
        "the interface changed; read_file shows the new skeleton"
    };
    let list = |v: &[String]| {
        if v.is_empty() {
            "none".to_owned()
        } else {
            v.join(", ")
        }
    };
    format!(
        "changed: {}; added: {}; removed: {}; {interface}",
        list(&changed),
        list(&added),
        list(&removed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalReader;
    use crate::model::{Item, Request};
    use crate::policy::Policy;
    use crate::testing::{Received, scripted_local};
    use crate::view::Presenter;
    use serde_json::Map;
    use std::sync::Arc;

    const CANARY: &str = "zq7c4kd92mx0vw3e";
    const SECRET_LINE: &str = "let uplift = base_rate * 0.8731 + seasonal_bias(region);";

    fn engine_src() -> String {
        format!(
            "//! Pricing engine.\n\n/// Volume discount in basis points.\npub fn discount_bps(qty: u32) -> u32 {{\n    \
             {SECRET_LINE}\n    let _rev = \"{CANARY}\";\n    if qty >= 200 {{ 750 }} else {{ 150 }}\n}}\n\n\
             /// Largest discount.\npub const MAX_BPS: u32 = 1200;\nconst REGION_KEY: &str = \"eu-core-7\";\n"
        )
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        ws: PathBuf,
        engine: Arc<Engine>,
    }

    fn fixture(local: Option<LocalReader>) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().canonicalize().unwrap().join("ws");
        for (p, text) in [
            ("src/pricing/engine.rs", engine_src()),
            (
                "src/pricing/table.rs",
                "pub fn table() -> u32 { 917 }\n".into(),
            ),
            ("src/invoice.rs", "pub fn total() -> u32 { 1 }\n".into()),
            ("data/tiers.csv", "id,tier\n1,G\n".into()),
        ] {
            let path = ws.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let policy = Policy {
            sensitive_globs: vec!["data/**".into()],
            interface_only: vec!["src/pricing/**".into()],
            sealed: vec!["src/pricing/table.rs".into()],
            command_output_sensitive: true,
            detect_secrets: true,
            detect_pii: true,
            ..Policy::default()
        };
        let engine = Engine::open(&dir.path().join("run"), policy, local).unwrap();
        let files: Vec<String> = [
            "src/pricing/engine.rs",
            "src/pricing/table.rs",
            "src/invoice.rs",
            "data/tiers.csv",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        engine.prime(&ws, &files, "Change the volume discount.");
        Fixture {
            _dir: dir,
            ws,
            engine,
        }
    }

    fn file(path: &str) -> Source {
        Source::File {
            path: path.into(),
            ranged: false,
        }
    }

    fn outbound(e: &Arc<Engine>, text: &str) -> String {
        let (filter, _) = e.outbound();
        let mut req = Request {
            items: vec![Item::ToolResult {
                call_id: "c".into(),
                content: text.to_owned(),
            }],
            ..Request::default()
        };
        filter.apply(&mut req);
        match &req.items[0] {
            Item::ToolResult { content, .. } => content.clone(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn interface_only_files_show_a_skeleton_with_stable_handles() {
        let f = fixture(None);
        let shown = f.engine.present(&file("src/pricing/engine.rs"), b"ignored");
        assert_eq!(f.engine.take_view_class(), Some(ViewClass::Protected));
        for kept in [
            "/// Volume discount in basis points.",
            "pub fn discount_bps(qty: u32) -> u32 { ⟨body:h",
            "pub const MAX_BPS: u32 = 1200;",
            "const REGION_KEY: &str = ⟨value:h",
            "edit_protected",
        ] {
            assert!(shown.contains(kept), "missing {kept:?}:\n{shown}");
        }
        for hidden in ["0.8731", CANARY, "750", "eu-core-7"] {
            assert!(!shown.contains(hidden), "{hidden} leaked:\n{shown}");
        }
        let again = f.engine.present(&file("src/pricing/engine.rs"), b"ignored");
        assert_eq!(shown, again, "handles are stable across reads");
    }

    #[test]
    fn sealed_files_show_existence_only() {
        let f = fixture(None);
        let shown = f
            .engine
            .present(&file("src/pricing/table.rs"), b"1  pub fn");
        assert!(shown.contains("is sealed") && shown.contains("ask_local(handle=\"h"));
        assert!(
            !shown.contains("917") && !shown.contains("pub fn table"),
            "{shown}"
        );
        let listed = f.engine.present(
            &Source::FileList,
            b"src/invoice.rs\nsrc/pricing/engine.rs\nsrc/pricing/table.rs\n",
        );
        assert!(
            listed.contains("src/pricing/table.rs  [sealed]"),
            "{listed}"
        );
        assert!(
            listed.contains("src/pricing/engine.rs  [interface-only]"),
            "{listed}"
        );
        assert!(listed.contains("src/invoice.rs\n"), "{listed}");
    }

    #[test]
    fn protected_source_is_hidden_from_commands_but_not_from_checks() {
        let f = fixture(None);
        let commands = f.engine.hidden_from_commands(&f.ws);
        assert!(commands.contains(&f.ws.join("src/pricing")), "{commands:?}");
        assert!(commands.contains(&f.ws.join("data")), "{commands:?}");
        let checks = f.engine.hidden_from_checks(&f.ws);
        assert!(
            !checks.iter().any(|p| p.starts_with(f.ws.join("src"))),
            "{checks:?}"
        );
        assert!(checks.contains(&f.ws.join("data")), "{checks:?}");
    }

    #[test]
    fn protected_files_cannot_be_written_directly() {
        let f = fixture(None);
        let err = f
            .engine
            .resolve_for_write(Path::new("src/pricing/engine.rs"), "x")
            .unwrap_err();
        assert!(err.contains("edit_protected"), "{err}");
        assert!(
            f.engine
                .resolve_for_write(Path::new("src/invoice.rs"), "x")
                .is_ok()
        );
    }

    #[test]
    fn copies_of_protected_code_do_not_cross_the_gate() {
        let f = fixture(None);
        // A compiler snippet quoting a withheld line.
        let rustc = format!("error[E0308]: mismatched types\n  12 |     {SECRET_LINE}\n");
        let sent = outbound(&f.engine, &rustc);
        assert!(
            !sent.contains("0.8731") && sent.contains("mismatched types"),
            "{sent}"
        );
        // A short quote of a withheld literal is replaced, and the check blocks the raw value.
        let sent = outbound(&f.engine, &format!("the revision is {CANARY}"));
        assert!(
            !sent.contains(CANARY) && sent.contains("⟨code:literal#"),
            "{sent}"
        );
        let (_, check) = f.engine.outbound();
        assert!(check.check(&json!({"x": CANARY})).is_err());
        // Open code and the skeleton's own lines pass unchanged.
        let open = "pub fn discount_bps(qty: u32) -> u32 {\npub fn total() -> u32 { 1 }\n";
        assert_eq!(outbound(&f.engine, open), open);
    }

    #[test]
    fn searches_listings_and_diffs_withhold_protected_content() {
        let f = fixture(None);
        let found = f.engine.present(
            &Source::Search {
                pattern: "uplift".into(),
            },
            format!(
                "src/pricing/engine.rs:5:     {SECRET_LINE}\nsrc/invoice.rs:1: pub fn total()\n"
            )
            .as_bytes(),
        );
        assert!(found.contains("src/pricing/engine.rs:5: [match in protected source]"));
        assert!(
            !found.contains("0.8731") && found.contains("src/invoice.rs:1:"),
            "{found}"
        );
        let diff = format!(
            "diff --git a/src/invoice.rs b/src/invoice.rs\n+pub fn total() -> u32 {{ 2 }}\n\
             diff --git a/src/pricing/engine.rs b/src/pricing/engine.rs\n+    {SECRET_LINE}\n"
        );
        let shown = f.engine.present(&Source::Diff, diff.as_bytes());
        assert!(shown.contains("+pub fn total() -> u32 { 2 }"), "{shown}");
        assert!(
            shown.contains("changes to protected source are not shown"),
            "{shown}"
        );
        assert!(!shown.contains("0.8731"), "{shown}");
    }

    #[test]
    fn runs_that_can_read_protected_code_show_result_lines_only() {
        let f = fixture(None);
        let out = format!(
            "exit code 101\n--- stdout ---\nrunning 1 test\n{SECRET_LINE}\n\
             sed-style error {SECRET_LINE}\ntest pricing::breaks ... FAILED\n\
             test result: FAILED. 0 passed; 1 failed\n"
        );
        let shown = f.engine.present(
            &Source::SensitiveCommand {
                command: "cargo test".into(),
                exit_code: Some(101),
            },
            out.as_bytes(),
        );
        assert_eq!(f.engine.take_view_class(), Some(ViewClass::HandleSummary));
        assert!(shown.contains("test pricing::breaks ... FAILED"), "{shown}");
        assert!(
            shown.contains("exit code 101") && shown.contains("ask_local"),
            "{shown}"
        );
        assert!(!shown.contains("0.8731"), "{shown}");
        let checks = f.engine.present(&Source::Checks, out.as_bytes());
        assert!(checks.contains("test result: FAILED") && !checks.contains("0.8731"));
    }

    #[test]
    fn the_task_names_protected_paths_and_only_ip_runs_get_edit_protected() {
        let f = fixture(None);
        let task = f.engine.sanitize_objective("Change the volume discount.");
        assert!(
            task.contains("src/pricing/engine.rs (interface-only)")
                && task.contains("src/pricing/table.rs (sealed)"),
            "{task}"
        );
        assert!(
            f.engine
                .extra_tools()
                .iter()
                .any(|t| t.name == "edit_protected")
        );
        let d = tempfile::tempdir().unwrap();
        let plain = Engine::open(d.path(), Policy::default(), None).unwrap();
        assert!(
            !plain
                .extra_tools()
                .iter()
                .any(|t| t.name == "edit_protected")
        );
        assert!(!plain.sanitize_objective("x").contains("Protected source"));
    }

    fn implement(f: &Fixture, spec: &str) -> Option<Result<Implemented, String>> {
        let current = std::fs::read_to_string(f.ws.join("src/pricing/engine.rs")).unwrap();
        f.engine.implement_protected(
            Path::new("src/pricing/engine.rs"),
            &current,
            &ImplementRequest {
                spec,
                tests: Some("#[test] fn t() { assert_eq!(discount_bps(1000), 1000); }"),
                feedback: None,
            },
        )
    }

    fn scripted(replies: &[String]) -> (LocalReader, Received) {
        scripted_local(replies.to_vec())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_local_model_implements_protected_changes() {
        let new_src = engine_src().replace(
            "if qty >= 200 { 750 }",
            "if qty >= 1000 { 1000 } else if qty >= 200 { 750 }",
        );
        let (local, received) = scripted(&[
            json!({"code": format!("```rust\n{new_src}```")}).to_string(),
            json!({"answer": "It uses a base rate.", "unanswerable": false}).to_string(),
        ]);
        let f = fixture(Some(local));
        let done = implement(&f, "Add a 1000-unit break worth 1000 bps.")
            .unwrap()
            .unwrap();
        assert_eq!(done.content, new_src);
        assert!(
            done.summary.contains("changed: fn discount_bps")
                && done.summary.contains(
                    "interface (signatures, types, doc comments, public constants) is unchanged"
                ),
            "{}",
            done.summary
        );
        let prompt = received.prompt(0);
        assert!(prompt.contains(SECRET_LINE) && prompt.contains("1000-unit break"));
        assert!(prompt.contains("<tests>"), "{prompt}");
        // The same schema as every other role (the server's prompt cache is keyed by it).
        let shown = f.engine.present(&file("src/pricing/table.rs"), b"");
        let id = shown
            .split("ask_local(handle=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let mut args = Map::new();
        args.insert("handle".into(), json!(id));
        args.insert("question".into(), json!("What does it return?"));
        let answer = f.engine.call_tool("ask_local", &args).unwrap().unwrap();
        assert!(answer.contains("base rate"), "{answer}");
        let bodies = received.bodies();
        assert!(bodies[0]["response_format"].is_object());
        assert_eq!(bodies[0]["response_format"], bodies[1]["response_format"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn implementations_that_do_not_parse_are_refused() {
        let (local, _) =
            scripted(&[json!({"code": "pub fn discount_bps(qty: u32 -> u32 {"}).to_string()]);
        let f = fixture(Some(local));
        let err = implement(&f, "anything").unwrap().unwrap_err();
        assert!(err.contains("was not written"), "{err}");
        assert!(
            f.engine
                .implement_protected(
                    Path::new("src/invoice.rs"),
                    "",
                    &ImplementRequest {
                        spec: "x",
                        tests: None,
                        feedback: None
                    }
                )
                .is_none(),
            "open files are not ours"
        );
    }
}
