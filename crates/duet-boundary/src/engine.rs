// SPDX-License-Identifier: GPL-3.0-or-later
//! The security engine: decides what the frontier sees of every tool result,
//! and sanitizes every outbound request.
//!
//! - Public content: shown, with detected secrets/PII replaced by placeholders.
//! - Secret-bearing files (`.env`-style): shown with every value replaced.
//! - Sensitive data, logs and command output: stored under a handle; the
//!   frontier gets error lines (sanitized) and a local-model summary, and can
//!   ask the local model questions.
//! - Public but bulky results (over `bulky_tokens`): stored under a handle;
//!   the frontier gets the first lines and an outline, and reads ranges with
//!   `read_raw`.
//! - Writes: placeholders are resolved locally, and secrets may only land in
//!   secret files.

use crate::bulky::{self, Shape};
use crate::detect::{Detectors, Kind, scan};
use crate::gate::{OutboundCheck, OutboundFilter};
use crate::handles::HandleStore;
use crate::local::LocalReader;
use crate::model::{Item, Request, ToolSpec};
use crate::overlap::OverlapIndex;
use crate::policy::{Policy, is_secret_bearing};
use crate::vault::Vault;
use crate::view::{Presenter, Source, ViewClass};
use duet_fs::FsError;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

static NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\p{Lu}\p{Ll}+(?:[ '-]\p{Lu}\p{Ll}+)+\b").expect("static regex")
});
// Values of person/address fields (`name=Cher`, `"display_name": "Bjørn Ødegaard"`), whatever their shape.
static PERSON_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b(?:full_?name|display_?name|first_?name|last_?name|given_?name|surname|user_?name|customer(?:_?name)?|contact(?:_?name)?|holder|recipient|name|street|address)["']?\s*[:=]\s*["']?([^|,;"'\n\t=&]{1,60})"#,
    )
    .expect("static regex")
});
static LONG_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{1,3}(?:,\d{3}){2,}\b|\b\d{6,}\b").expect("static regex"));
static ERROR_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(error|panic|panicked|exception|traceback|failed|failure|fatal|warn|warning|invalid|unwrap|denied|timeout)\b")
        .expect("static regex")
});
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{L}+").expect("static regex"));
static TERM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{L}\p{N}_]+").expect("static regex"));

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    TERM.find_iter(text).map(|m| m.as_str().to_lowercase())
}

/// Other forms the same value takes once code touches it: a person's surname
/// alone; a number without or with digit grouping, or read as minor units.
fn other_spellings(value: &str, kind: Kind) -> Vec<String> {
    match kind {
        Kind::Name => {
            let parts: Vec<&str> = WORD.find_iter(value).map(|m| m.as_str()).collect();
            match parts.last() {
                Some(last) if parts.len() >= 2 && last.chars().count() >= 4 => {
                    vec![(*last).to_owned()]
                }
                _ => Vec::new(),
            }
        }
        Kind::Data if value.bytes().all(|b| b.is_ascii_digit() || b == b',') => {
            let digits: String = value.chars().filter(char::is_ascii_digit).collect();
            if digits.len() < 6 {
                return Vec::new();
            }
            let (units, cents) = digits.split_at(digits.len() - 2);
            vec![
                digits.clone(),
                group(&digits),
                format!("{units}.{cents}"),
                format!("{}.{cents}", group(units)),
            ]
        }
        _ => Vec::new(),
    }
}

fn group(digits: &str) -> String {
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Directories never scanned for sensitive files (build output, dependencies, state).
const COMMAND_SCAN_SKIP: &[&str] = &[".git", ".duet", "target", "node_modules"];

/// Words that look like names in title case but are not personal data.
const NAME_STOPWORDS: &[&str] = &[
    "Result", "Option", "Error", "String", "Vec", "Some", "None", "Ok", "Err", "Self", "Warn",
    "Info", "Debug",
];

struct State {
    vault: Vault,
    handles: HandleStore,
    overlap: OverlapIndex,
    /// Lower-cased words of public files and the task: a single word found
    /// here is never treated as identifying on its own.
    public_words: std::collections::HashSet<String>,
    /// Sensitive files found when priming, for the note added to the task.
    sensitive_files: Vec<String>,
}

pub struct Engine {
    policy: Policy,
    detectors: Detectors,
    local: Option<LocalReader>,
    state: Mutex<State>,
    /// Files created or changed by commands that could read sensitive data.
    derived: Mutex<std::collections::HashSet<std::path::PathBuf>>,
    /// Where `derived` is persisted, so a resumed run keeps it.
    derived_file: std::path::PathBuf,
    /// How the latest result was shown (for the cost ledger).
    last_class: Mutex<Option<ViewClass>>,
}

pub const MAX_KEY_LINES: usize = 12;
/// Command output up to this size is shown sanitized instead of summarized.
pub const INLINE_OUTPUT_CHARS: usize = 6000;
/// Questions answered per `ask_local` call.
pub const MAX_QUESTIONS: usize = 6;
/// Lines `read_raw` returns when no end is given, and at most per call.
pub const DEFAULT_RAW_LINES: usize = 200;
pub const MAX_RAW_LINES: usize = 500;
/// Sensitive paths named in the task note; beyond this the note lists their directories.
const LISTED_PATHS: usize = 20;
/// Sensitive files larger than this are not pre-indexed.
pub const PRIME_MAX_BYTES: usize = 2 * 1024 * 1024;

impl Engine {
    pub fn open(
        run_dir: &Path,
        policy: Policy,
        local: Option<LocalReader>,
    ) -> Result<Arc<Self>, FsError> {
        let detectors = Detectors {
            secrets: policy.detect_secrets,
            pii: policy.detect_pii,
            entropy: policy.detect_entropy,
        };
        Ok(Arc::new(Self {
            policy,
            detectors,
            local,
            derived: Mutex::new(
                std::fs::read(run_dir.join("derived.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default(),
            ),
            derived_file: run_dir.join("derived.json"),
            last_class: Mutex::new(None),
            state: Mutex::new(State {
                vault: Vault::open(&run_dir.join("vault.json"))?,
                handles: HandleStore::open(&run_dir.join("handles"))?,
                overlap: OverlapIndex::default(),
                public_words: Default::default(),
                sensitive_files: Vec::new(),
            }),
        }))
    }

    /// Indexes every sensitive file before the run starts: its values seed the
    /// vault and its text seeds the copied-span index, so later echoes of that
    /// content (`cat`, test output, logs) are replaced wherever they appear.
    /// Public files and the task text are read first: their words decide which
    /// single words (a surname, a reformatted number) may count as identifying.
    pub fn prime(&self, workspace: &Path, files: &[String], objective: &str) -> usize {
        {
            let mut st = self.lock();
            st.public_words.extend(words(objective));
            for f in files {
                let path = Path::new(f);
                if self.is_sensitive(path) {
                    continue;
                }
                if let Ok(bytes) = duet_fs::read_file(workspace, path, PRIME_MAX_BYTES as u64) {
                    st.public_words
                        .extend(words(&String::from_utf8_lossy(&bytes)));
                }
            }
        }
        let mut primed = 0;
        for f in files {
            let path = Path::new(f);
            if !self.is_sensitive(path) {
                continue;
            }
            let Ok(bytes) = duet_fs::read_file(workspace, path, PRIME_MAX_BYTES as u64) else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            let mut st = self.lock();
            st.sensitive_files.push(f.clone());
            st.overlap.add_sensitive(&text);
            if is_secret_bearing(path) {
                let _ = self.tokenized_view(&mut st, f, &text);
            } else {
                let _ = self.sanitize(&mut st, &text, f, true);
            }
            primed += 1;
        }
        primed
    }

    /// Sensitive by policy, or derived from sensitive data by a command.
    fn is_sensitive(&self, path: &Path) -> bool {
        self.policy.is_sensitive_path(path)
            || self
                .derived
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(path)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Replaces detected secrets/PII (and, in sensitive context, names and long
    /// numbers) with placeholders, then any value already in the vault.
    fn sanitize(&self, st: &mut State, text: &str, origin: &str, sensitive: bool) -> String {
        let mut spans: Vec<(usize, usize, Kind, Option<String>)> = scan(text, self.detectors)
            .into_iter()
            .map(|f| (f.start, f.end, f.kind, f.label))
            .collect();
        if sensitive {
            for m in NAME.find_iter(text) {
                // A stop word splits the run; each remaining run of 2+ words is a name.
                let mut run: Option<(usize, usize, usize)> = None; // (start, end, words)
                let mut flush = |run: &mut Option<(usize, usize, usize)>| {
                    if let Some((s, e, n)) = run.take()
                        && n >= 2
                    {
                        spans.push((s, e, Kind::Name, None));
                    }
                };
                for w in WORD.find_iter(m.as_str()) {
                    let (s, e) = (m.start() + w.start(), m.start() + w.end());
                    if NAME_STOPWORDS.contains(&w.as_str()) {
                        flush(&mut run);
                    } else {
                        run = Some(run.map_or((s, e, 1), |(rs, _, n)| (rs, e, n + 1)));
                    }
                }
                flush(&mut run);
            }
            for c in PERSON_FIELD.captures_iter(text) {
                let Some(v) = c.get(1) else { continue };
                let trimmed = v.as_str().trim_end();
                if trimmed.trim().is_empty() || trimmed.starts_with(['⟨', '<', '$', '{', '(']) {
                    continue;
                }
                spans.push((v.start(), v.start() + trimmed.len(), Kind::Name, None));
            }
            for m in LONG_NUMBER.find_iter(text) {
                spans.push((m.start(), m.end(), Kind::Data, None));
            }
        }
        spans.sort_by_key(|s| (s.0, std::cmp::Reverse(s.1)));
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for (start, end, kind, label) in spans {
            if start < last {
                continue;
            }
            let value = &text[start..end];
            let token = st
                .vault
                .token_for(value, kind, label.as_deref(), origin)
                .unwrap_or_else(|_| format!("⟨{}⟩", kind.tag()));
            if sensitive {
                for spelling in other_spellings(value, kind) {
                    if !st.public_words.contains(&spelling.to_lowercase()) {
                        let _ = st.vault.alias(&spelling, value);
                    }
                }
            }
            out.push_str(&text[last..start]);
            out.push_str(&token);
            last = end;
        }
        out.push_str(&text[last..]);
        st.vault.tokenize(&out).0
    }

    /// `KEY=value` files: structure kept, every value replaced.
    fn tokenized_view(&self, st: &mut State, path: &str, text: &str) -> String {
        let mut out = String::new();
        for line in text.lines() {
            let trimmed = line.trim_start();
            let body = trimmed.strip_prefix("export ").unwrap_or(trimmed);
            match body.split_once('=') {
                Some((key, value)) if !trimmed.starts_with('#') && !value.trim().is_empty() => {
                    let v = value.trim().trim_matches(|c| c == '"' || c == '\'');
                    let token = st
                        .vault
                        .token_for(v, Kind::Secret, Some(key.trim()), path)
                        .unwrap_or_default();
                    let prefix = &line[..line.len() - value.len()];
                    out.push_str(&format!("{prefix}{token}\n"));
                }
                _ => {
                    out.push_str(&self.sanitize(st, line, path, true));
                    out.push('\n');
                }
            }
        }
        format!(
            "[values in this file are shown as placeholders; write them back verbatim and they are restored locally]\n{out}"
        )
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(f))
    }

    /// Sensitive content: handle, sanitized error lines, local summary.
    fn handle_view(&self, source_label: &str, text: &str) -> String {
        self.set_class(ViewClass::HandleSummary);
        let handle = {
            let mut st = self.lock();
            st.overlap.add_sensitive(text);
            match st.handles.put(text.as_bytes(), source_label) {
                Ok(h) => h,
                Err(e) => return format!("[content withheld: could not store it locally: {e}]"),
            }
        };
        let lines: Vec<&str> = text.lines().collect();
        let error_lines: Vec<(usize, &str)> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| ERROR_LINE.is_match(l))
            .map(|(i, l)| (i + 1, *l))
            .collect();
        let digest = self
            .local
            .as_ref()
            .map(|l| Self::block_on(l.digest(source_label, text)));
        let mut st = self.lock();
        let mut out = format!(
            "{} ({}): {} lines, {} bytes, {} line(s) mention errors. Raw content stays on this machine; \
ask questions with ask_local(handle=\"{}\", question=...).\n",
            handle.id,
            source_label,
            lines.len(),
            text.len(),
            error_lines.len(),
            handle.id
        );
        if !error_lines.is_empty() {
            out.push_str("Error lines (sensitive values replaced):\n");
            for (n, l) in error_lines.iter().take(MAX_KEY_LINES) {
                let shown: String = l.chars().take(300).collect();
                out.push_str(&format!(
                    "{n:>6}  {}\n",
                    self.sanitize(&mut st, &shown, source_label, true)
                ));
            }
            if error_lines.len() > MAX_KEY_LINES {
                out.push_str(&format!("  … {} more\n", error_lines.len() - MAX_KEY_LINES));
            }
        }
        match digest {
            Some(Ok(d)) => {
                out.push_str(&format!(
                    "Summary by the local model: {}\n",
                    self.clean_local(&mut st, &d.summary, source_label)
                ));
                for f in &d.facts {
                    out.push_str(&format!(
                        "- {}\n",
                        self.clean_local(&mut st, f, source_label)
                    ));
                }
            }
            Some(Err(e)) => out.push_str(&format!(
                "[local summary unavailable: {}]\n",
                e.message.chars().take(160).collect::<String>()
            )),
            None => {
                out.push_str("[no local model configured; only the lines above are available]\n")
            }
        }
        out
    }

    /// Command output: small output is shown with known values, detected
    /// secrets/PII and copied sensitive text replaced (no local call); large
    /// output goes to a handle with a summary.
    fn command_view(&self, label: &str, text: &str) -> String {
        if text.len() > INLINE_OUTPUT_CHARS {
            return self.handle_view(label, text);
        }
        let mut st = self.lock();
        let s = self.sanitize(&mut st, text, label, false);
        st.overlap.redact(&s).0
    }

    /// Local-model output: sanitized as sensitive text, then copied spans removed.
    fn clean_local(&self, st: &mut State, text: &str, origin: &str) -> String {
        let s = self.sanitize(st, text, origin, true);
        st.overlap.redact(&s).0
    }

    /// Public text as it may be shown: detected values replaced, copied
    /// sensitive spans removed.
    fn clean_public(&self, st: &mut State, text: &str, origin: &str) -> String {
        let s = self.sanitize(st, text, origin, false);
        st.overlap.redact(&s).0
    }

    fn set_class(&self, class: ViewClass) {
        *self
            .last_class
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(class);
    }

    fn offload(&self, text: &str) -> bool {
        bulky::is_bulky(text, self.policy.bulky_tokens)
    }

    /// Public but bulky content (`PublicBulky`): kept whole under a handle the
    /// frontier may read ranges of; it gets a deterministic head and outline,
    /// plus a local summary of files and command output when a local model is
    /// configured. `text` from `read_file` is numbered; the handle keeps plain
    /// lines and the file's own line numbers.
    fn bulky_view(&self, label: &str, text: &str, shape: Shape) -> String {
        let (first, body) = match shape {
            Shape::Source => {
                bulky::strip_line_numbers(text).unwrap_or_else(|| (1, text.to_owned()))
            }
            _ => (1, text.to_owned()),
        };
        let stored = self
            .lock()
            .handles
            .put_public(body.as_bytes(), label, first);
        let Ok(handle) = stored else {
            // Nowhere to keep it: show it whole, as a small public result.
            let mut st = self.lock();
            return self.clean_public(&mut st, text, label);
        };
        self.set_class(ViewClass::BulkyHandle);
        let digest = match (shape, &self.local) {
            (Shape::Source | Shape::Output, Some(l)) => {
                Some(Self::block_on(l.digest(label, &body)))
            }
            _ => None,
        };
        let id = &handle.id;
        let last = first + body.lines().count().saturating_sub(1);
        let also = if shape == Shape::Source {
            " (or read_file with start_line/end_line)"
        } else {
            ""
        };
        let mut out = format!(
            "{id} ({label}): lines {first}-{last}, about {} tokens; public, but too long to show whole. \
Read any range with read_raw(handle=\"{id}\", start_line=..., end_line=...){also}.\n",
            bulky::tokens(&body)
        );
        let mut st = self.lock();
        out.push_str(&self.clean_public(&mut st, &bulky::preview(&body, first, shape), label));
        match digest {
            Some(Ok(d)) => {
                out.push_str(&format!(
                    "Summary by the local model: {}\n",
                    self.clean_public(&mut st, &d.summary, label)
                ));
                for f in &d.facts {
                    out.push_str(&format!("- {}\n", self.clean_public(&mut st, f, label)));
                }
            }
            Some(Err(e)) => out.push_str(&format!(
                "[local summary unavailable: {}]\n",
                e.message.chars().take(160).collect::<String>()
            )),
            None => {}
        }
        out
    }

    fn search_view(&self, text: &str) -> String {
        let mut st = self.lock();
        let mut out = String::new();
        for line in text.lines() {
            let path = line.split(':').next().unwrap_or_default();
            if !path.is_empty() && self.is_sensitive(Path::new(path)) {
                let loc: String = line.splitn(3, ':').take(2).collect::<Vec<_>>().join(":");
                out.push_str(&format!(
                    "{loc}: [match in sensitive content; read_file gives a summary]\n"
                ));
            } else {
                out.push_str(&self.sanitize(&mut st, line, "search", false));
                out.push('\n');
            }
        }
        out
    }

    fn ask_local(&self, args: &Map<String, Value>) -> Result<String, String> {
        let id = args
            .get("handle")
            .and_then(Value::as_str)
            .ok_or("missing `handle`")?;
        // Several questions in one call cost one frontier turn, and the local
        // server reuses the processed content between them.
        let questions: Vec<&str> = match args.get("questions").and_then(Value::as_array) {
            Some(qs) => qs.iter().filter_map(Value::as_str).collect(),
            None => args
                .get("question")
                .and_then(Value::as_str)
                .into_iter()
                .collect(),
        };
        if questions.is_empty() {
            return Err("give `questions` (a list) or `question`".into());
        }
        let (info, bytes) = self
            .lock()
            .handles
            .get(id)
            .ok_or_else(|| format!("unknown handle {id}"))?;
        let local = self.local.as_ref().ok_or("no local model is configured")?;
        let text = String::from_utf8_lossy(&bytes);
        let mut out = Vec::new();
        for (i, question) in questions.iter().take(MAX_QUESTIONS).enumerate() {
            let q = self.detokenize(question);
            let a = Self::block_on(local.answer(&info.source, &text, &q))
                .map_err(|e| format!("local model: {}", e.message))?;
            let mut st = self.lock();
            let answer = self.clean_local(&mut st, &a.answer, &info.source);
            let body = if a.unanswerable {
                format!("The local model could not answer from {id}. {answer}")
            } else {
                format!("{answer}\n(evidence lines: {:?})", a.evidence_lines)
            };
            out.push(if questions.len() > 1 {
                format!("{}. {body}", i + 1)
            } else {
                body
            });
        }
        if questions.len() > MAX_QUESTIONS {
            out.push(format!(
                "(only the first {MAX_QUESTIONS} questions were answered)"
            ));
        }
        Ok(out.join("\n\n"))
    }

    fn read_raw(&self, args: &Map<String, Value>) -> Result<String, String> {
        let id = args
            .get("handle")
            .and_then(Value::as_str)
            .ok_or("missing `handle`")?;
        let (info, bytes) = self
            .lock()
            .handles
            .get(id)
            .ok_or_else(|| format!("unknown handle {id}"))?;
        if !info.public {
            return Err(format!(
                "{id} holds sensitive content; use ask_local instead"
            ));
        }
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.lines().collect();
        let first = info.first_line;
        if lines.is_empty() {
            return Err(format!("{id} is empty"));
        }
        let last = first + lines.len() - 1;
        let start = args
            .get("start_line")
            .and_then(Value::as_u64)
            .map_or(first, |s| s as usize)
            .max(first);
        if start > last {
            return Err(format!("{id} holds lines {first}-{last}"));
        }
        let requested_end = args
            .get("end_line")
            .and_then(Value::as_u64)
            .map_or(start + DEFAULT_RAW_LINES - 1, |e| e as usize);
        if requested_end < start {
            return Err(format!(
                "end_line {requested_end} is before start_line {start}"
            ));
        }
        let end = requested_end.min(last).min(start + MAX_RAW_LINES - 1);
        let mut chunk = format!(
            "{id} ({}), lines {start}-{end} of {first}-{last}:\n",
            info.source
        );
        for (i, l) in lines[start - first..=end - first].iter().enumerate() {
            chunk.push_str(&format!("{:>6}  {l}\n", start + i));
        }
        if end < requested_end.min(last) {
            chunk.push_str(&format!(
                "[at most {MAX_RAW_LINES} lines per call; continue from line {}]\n",
                end + 1
            ));
        }
        let mut st = self.lock();
        Ok(self.clean_public(&mut st, &chunk, &info.source))
    }

    /// What the local model did since the last call (`None` without a local model).
    pub fn take_local_stats(&self) -> Option<crate::local::CallStats> {
        self.local.as_ref().map(LocalReader::take_stats)
    }

    /// The outbound filter and check backed by this engine.
    pub fn outbound(self: &Arc<Self>) -> (Box<dyn OutboundFilter>, Box<dyn OutboundCheck>) {
        (
            Box::new(Sanitize(self.clone())),
            Box::new(NoKnownValues(self.clone())),
        )
    }
}

impl Presenter for Engine {
    fn present(&self, source: &Source, bytes: &[u8]) -> String {
        let text = String::from_utf8_lossy(bytes);
        self.set_class(ViewClass::Raw);
        match source {
            Source::File { path, .. } if self.is_sensitive(path) => {
                let label = path.display().to_string();
                if is_secret_bearing(path) {
                    self.set_class(ViewClass::Tokenized);
                    let mut st = self.lock();
                    st.overlap.add_sensitive(&text);
                    self.tokenized_view(&mut st, &label, &text)
                } else {
                    self.handle_view(&label, &text)
                }
            }
            Source::File { path, ranged } => {
                let label = path.display().to_string();
                self.lock().overlap.add_public(&text);
                // A range the model asked for is shown unless it is longer
                // than one `read_raw` call returns.
                let offload = if *ranged {
                    text.lines().count() > MAX_RAW_LINES
                } else {
                    self.offload(&text)
                };
                if offload {
                    return self.bulky_view(&label, &text, Shape::Source);
                }
                let mut st = self.lock();
                self.sanitize(&mut st, &text, &label, false)
            }
            Source::Command { command, .. } | Source::Other { label: command }
                if self.policy.command_output_sensitive
                    && !self.policy.command_is_raw_ok(command) =>
            {
                self.command_view(&format!("output of `{command}`"), &text)
            }
            Source::SensitiveCommand { command, .. } => {
                let label = format!("output of `{command}` (ran with sensitive data)");
                {
                    let mut st = self.lock();
                    st.overlap.add_sensitive(&text);
                }
                self.handle_view(&label, &text)
            }
            Source::Checks if self.policy.command_output_sensitive => {
                self.command_view("check output", &text)
            }
            // Public command output (allowlisted commands, or all output when
            // command output is not treated as sensitive).
            Source::Command { command, .. } | Source::Other { label: command }
                if self.offload(&text) =>
            {
                self.bulky_view(&format!("output of `{command}`"), &text, Shape::Output)
            }
            Source::Checks if self.offload(&text) => {
                self.bulky_view("check output", &text, Shape::Output)
            }
            Source::FileList if self.offload(&text) => {
                self.bulky_view("file list", &text, Shape::Listing)
            }
            // Matches in sensitive files are masked first; only that view is kept.
            Source::Search { pattern } => {
                let view = self.search_view(&text);
                if self.offload(&view) {
                    self.bulky_view(&format!("search for `{pattern}`"), &view, Shape::Matches)
                } else {
                    view
                }
            }
            _ => {
                let mut st = self.lock();
                self.sanitize(&mut st, &text, "tool output", false)
            }
        }
    }

    fn hidden_from_commands(&self, workspace: &Path) -> Vec<std::path::PathBuf> {
        let mut out: Vec<std::path::PathBuf> = self
            .policy
            .sensitive_globs
            .iter()
            .chain(&self.policy.protected_paths)
            .filter_map(|g| g.strip_suffix("/**"))
            .filter(|dir| !dir.contains(['*', '?', '[']))
            .map(|dir| workspace.join(dir))
            .filter(|p| p.is_dir())
            .collect();
        let mut stack = vec![std::path::PathBuf::new()];
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
                    Ok(t) if t.is_file() && self.is_sensitive(&child) => out.push(abs),
                    _ => {}
                }
            }
        }
        out.sort();
        out
    }

    fn mark_sensitive(&self, workspace: &Path, paths: &[std::path::PathBuf]) {
        for rel in paths {
            if self.is_sensitive(rel) {
                continue;
            }
            {
                let mut derived = self
                    .derived
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                derived.insert(rel.clone());
                let _ = duet_fs::private::write_private(
                    &self.derived_file,
                    &serde_json::to_vec(&*derived).unwrap_or_default(),
                );
            }
            if let Ok(bytes) = duet_fs::read_file(workspace, rel, PRIME_MAX_BYTES as u64) {
                let text = String::from_utf8_lossy(&bytes);
                let mut st = self.lock();
                st.overlap.add_sensitive(&text);
                let _ = self.sanitize(&mut st, &text, &rel.display().to_string(), true);
            }
        }
    }

    fn extra_tools(&self) -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: "run_command".into(),
                description: "Run a shell command in the repository root (sandboxed: no network, writes limited to the \
repository). Sensitive files (data, logs, secrets) are unreadable to commands. To run something that must read them \
(e.g. the program on the real data), set sensitive_data: the output then stays on this machine and you get a summary \
and a handle for ask_local, and files the command writes become sensitive too. Prefer synthetic fixtures for tests."
                    .into(),
                parameters: json!({"type": "object", "properties": {
                    "command": {"type": "string"},
                    "timeout_seconds": {"type": "integer", "minimum": 1},
                    "sensitive_data": {"type": "boolean", "description": "Allow the command to read sensitive files."}
                }, "required": ["command"]}),
            },
            ToolSpec {
                name: "ask_local".into(),
                description: "Ask the local model about content held under a handle (sensitive files, logs, \
command output). It reads the raw content on this machine and answers without revealing sensitive values. \
Put everything you need to know about one handle in a single call."
                    .into(),
                parameters: json!({"type": "object", "properties": {
                    "handle": {"type": "string", "description": "A handle such as h3."},
                    "questions": {"type": "array", "items": {"type": "string"}, "maxItems": MAX_QUESTIONS,
                        "description": "One or more questions, answered in order."},
                    "question": {"type": "string", "description": "A single question (alternative to questions)."}
                }, "required": ["handle"]}),
            },
            ToolSpec {
                name: "read_raw".into(),
                description: format!(
                    "Read a line range of a large public result held under a handle (line numbers as shown; \
at most {MAX_RAW_LINES} lines per call)."
                ),
                parameters: json!({"type": "object", "properties": {
                    "handle": {"type": "string"},
                    "start_line": {"type": "integer", "minimum": 1},
                    "end_line": {"type": "integer", "minimum": 1}
                }, "required": ["handle"]}),
            },
        ]
    }

    fn call_tool(&self, name: &str, args: &Map<String, Value>) -> Option<Result<String, String>> {
        let (result, class) = match name {
            "ask_local" => (self.ask_local(args), ViewClass::LocalAnswer),
            "read_raw" => (self.read_raw(args), ViewClass::Raw),
            _ => return None,
        };
        if result.is_ok() {
            self.set_class(class);
        }
        Some(result)
    }

    fn take_view_class(&self) -> Option<ViewClass> {
        self.last_class
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    fn detokenize(&self, text: &str) -> String {
        self.lock().vault.detokenize(text).0
    }

    fn resolve_for_write(&self, path: &Path, text: &str) -> Result<String, String> {
        let st = self.lock();
        let sink = self.policy.is_secret_sink(path) || self.is_sensitive(path);
        for token in Vault::tokens_in(text) {
            match st.vault.value_of(&token) {
                None => {
                    return Err(format!(
                        "{token} is not a known placeholder; it cannot be written"
                    ));
                }
                Some((_, entry)) if !sink => {
                    let key = token
                        .trim_matches(|c| c == '⟨' || c == '⟩')
                        .split(':')
                        .nth(1)
                        .unwrap_or("VALUE")
                        .split('#')
                        .next()
                        .unwrap_or("VALUE")
                        .to_owned();
                    return Err(format!(
                        "{token} holds a {} value and may only be written into secret files ({}); \
in {} read it at runtime instead (for example from the environment variable {key}).",
                        entry.kind.tag(),
                        self.policy.secret_sinks.join(", "),
                        path.display()
                    ));
                }
                Some(_) => {}
            }
        }
        Ok(st.vault.detokenize(text).0)
    }

    /// The task, sanitized, with a note naming the sensitive paths, so the model
    /// reaches for summaries and `sensitive_data` instead of hitting denials.
    fn sanitize_objective(&self, text: &str) -> String {
        let mut st = self.lock();
        let mut out = self.sanitize(&mut st, text, "task", false);
        if !st.sensitive_files.is_empty() {
            let mut paths = st.sensitive_files.clone();
            if paths.len() > LISTED_PATHS {
                paths = paths
                    .iter()
                    .map(|p| match p.rsplit_once('/') {
                        Some((dir, _)) => format!("{dir}/"),
                        None => p.clone(),
                    })
                    .collect();
                paths.dedup();
                paths.truncate(LISTED_PATHS);
            }
            out.push_str(&format!(
                "\n\nSensitive in this repository (commands cannot read these; read_file gives a summary and a \
handle for ask_local; run_command with sensitive_data runs programs on them): {}",
                paths.join(", ")
            ));
        }
        out
    }
}

/// Outbound filter: sanitizes every item again (idempotent). The model's own
/// messages get known values replaced too: it can reconstruct a value it never
/// saw verbatim (from character codes, a reformatted number), and history must
/// not carry that value back out.
struct Sanitize(Arc<Engine>);

impl OutboundFilter for Sanitize {
    fn name(&self) -> &'static str {
        "sanitize"
    }

    fn apply(&self, request: &mut Request) -> Vec<String> {
        let mut notes = Vec::new();
        let mut st = self.0.lock();
        for item in &mut request.items {
            let text = match item {
                Item::User { text } => text,
                Item::ToolResult { content, .. } => content,
                Item::Assistant {
                    text,
                    reasoning,
                    tool_calls,
                } => {
                    let mut replaced = 0;
                    for t in std::iter::once(text)
                        .chain(reasoning.iter_mut())
                        .chain(tool_calls.iter_mut().map(|c| &mut c.raw_arguments))
                    {
                        let (cleaned, n) = st.vault.tokenize(t);
                        if n > 0 {
                            *t = cleaned;
                            replaced += n;
                        }
                    }
                    for call in tool_calls.iter_mut() {
                        if let Ok(serde_json::Value::Object(args)) =
                            serde_json::from_str(&call.raw_arguments)
                        {
                            call.arguments = args;
                        }
                    }
                    if replaced > 0 {
                        notes.push(format!(
                            "replaced {replaced} known value(s) in the model's own message"
                        ));
                    }
                    continue;
                }
            };
            let cleaned = self.0.sanitize(&mut st, text, "outbound", false);
            let (cleaned, spans) = st.overlap.redact(&cleaned);
            if cleaned != *text {
                notes.push(format!(
                    "replaced sensitive content ({spans} copied span(s))"
                ));
                *text = cleaned;
            }
        }
        notes
    }
}

/// Final check: no value in the vault may appear anywhere in the body.
struct NoKnownValues(Arc<Engine>);

impl OutboundCheck for NoKnownValues {
    fn name(&self) -> &'static str {
        "known-values"
    }

    fn check(&self, body: &Value) -> Result<(), String> {
        let text = body.to_string();
        let st = self.0.lock();
        for (value, entry) in st.vault.values() {
            if value.len() >= 6
                && (text.contains(value)
                    || text.contains(
                        &serde_json::to_string(value)
                            .unwrap_or_default()
                            .trim_matches('"')
                            .to_owned(),
                    ))
            {
                return Err(format!(
                    "a {} value from {} would have been sent",
                    entry.kind.tag(),
                    entry.origin
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const KEY: &str = "sk_live_Qm8vT2xW9pL4nR7kZ3cY6bH1";
    const PASSWORD: &str = "Ab3Xy9Qw!p42Lm";
    const EMAIL: &str = "amelia.velanwick42@mailbox-311.net";

    fn policy() -> Policy {
        Policy {
            sensitive_globs: vec![
                ".env*".into(),
                "data/**".into(),
                "logs/**".into(),
                "*.log".into(),
            ],
            command_output_sensitive: true,
            raw_ok_commands: vec!["cargo check".into()],
            secret_sinks: vec![".env*".into(), "config/*.toml".into()],
            detect_secrets: true,
            detect_pii: true,
            detect_entropy: true,
            bulky_tokens: 2000,
            ..Policy::default()
        }
    }

    fn engine() -> (tempfile::TempDir, Arc<Engine>) {
        let d = tempfile::tempdir().unwrap();
        let e = Engine::open(d.path(), policy(), None).unwrap();
        (d, e)
    }

    fn file(path: &str) -> Source {
        Source::File {
            path: PathBuf::from(path),
            ranged: false,
        }
    }

    #[test]
    fn env_files_show_structure_not_values() {
        let (_d, e) = engine();
        let env =
            format!("# creds\nPAYMENTS_API_KEY={KEY}\nexport LEDGER_DB_PASSWORD=\"{PASSWORD}\"\n");
        let shown = e.present(&file(".env"), env.as_bytes());
        assert!(!shown.contains(KEY) && !shown.contains(PASSWORD), "{shown}");
        assert!(
            shown.contains("PAYMENTS_API_KEY=⟨secret:PAYMENTS_API_KEY#1⟩"),
            "{shown}"
        );
        assert!(shown.contains("# creds"));
    }

    #[test]
    fn logs_become_a_handle_with_sanitized_error_lines() {
        let (_d, e) = engine();
        let log = format!(
            "INFO start\nERROR panicked parsing record for Amelia Velanwick <{EMAIL}> amount 4,812,339 from 203.0.113.41\nINFO done\n"
        );
        let shown = e.present(&file("logs/prod.log"), log.as_bytes());
        assert!(shown.starts_with("h1 (logs/prod.log)"), "{shown}");
        for secret in [
            EMAIL,
            "Amelia Velanwick",
            "Velanwick",
            "4,812,339",
            "203.0.113.41",
        ] {
            assert!(!shown.contains(secret), "{secret} leaked: {shown}");
        }
        assert!(shown.contains("panicked parsing record"), "{shown}");
    }

    #[test]
    fn person_fields_are_replaced_whatever_their_shape() {
        let (_d, e) = engine();
        let log = "ERROR panicked, last input: 2026-09-14T09:02:17Z|user=x|name=Madonnaquist|amount=3\n\
            ERROR bad row {\"display_name\": \"bjørn ødegaard\", \"street\": \"12 Elm Rd\"}\n";
        let shown = e.present(&file("logs/prod.log"), log.as_bytes());
        for value in ["Madonnaquist", "bjørn ødegaard", "12 Elm Rd"] {
            assert!(!shown.contains(value), "{value} leaked: {shown}");
        }
        assert!(shown.contains("name=⟨name:"), "{shown}");
        // The same value echoed later anywhere else is replaced too.
        let echo = e.present(&file("src/lib.rs"), b"// e.g. Madonnaquist\n");
        assert!(!echo.contains("Madonnaquist"), "{echo}");
    }

    #[test]
    fn secrets_in_public_source_are_replaced() {
        let (_d, e) = engine();
        let src = format!("const KEY: &str = \"{KEY}\";\nfn main() {{}}\n");
        let shown = e.present(&file("src/main.rs"), src.as_bytes());
        assert!(!shown.contains(KEY));
        assert!(shown.contains("fn main()"));
    }

    #[test]
    fn command_output_is_sensitive_unless_allowlisted() {
        let (_d, e) = engine();
        let out = format!("error: failed to connect as {EMAIL}\n");
        let shown = e.present(
            &Source::Command {
                command: "cargo test".into(),
                exit_code: Some(1),
            },
            out.as_bytes(),
        );
        assert!(
            !shown.contains(EMAIL) && shown.contains("failed to connect as"),
            "{shown}"
        );
        let big = format!("ERROR {EMAIL}\n{}", "noise line\n".repeat(1000));
        let handled = e.present(
            &Source::Command {
                command: "cargo test".into(),
                exit_code: Some(1),
            },
            big.as_bytes(),
        );
        assert!(
            handled.contains("ask_local") && !handled.contains(EMAIL),
            "{handled}"
        );
        let raw = e.present(
            &Source::Command {
                command: "cargo check".into(),
                exit_code: Some(0),
            },
            b"Checking fx v0.1.0\n",
        );
        assert!(raw.contains("Checking fx"), "{raw}");
    }

    #[test]
    fn secrets_are_restored_only_in_secret_files() {
        let (_d, e) = engine();
        e.present(
            &file(".env"),
            format!("PAYMENTS_API_KEY={KEY}\n").as_bytes(),
        );
        let token = "⟨secret:PAYMENTS_API_KEY#1⟩";
        let into_env = e
            .resolve_for_write(
                Path::new(".env"),
                &format!("PAYMENTS_API_KEY={token}\nNEW=1\n"),
            )
            .unwrap();
        assert!(into_env.contains(KEY));
        let into_code =
            e.resolve_for_write(Path::new("src/client.rs"), &format!("let k = \"{token}\";"));
        let err = into_code.unwrap_err();
        assert!(
            err.contains("environment variable PAYMENTS_API_KEY"),
            "{err}"
        );
        assert!(
            e.resolve_for_write(Path::new("src/a.rs"), "⟨secret:NOPE#1⟩")
                .is_err()
        );
        assert_eq!(e.detokenize(&format!("old {token}")), format!("old {KEY}"));
    }

    #[test]
    fn outbound_filter_and_check_stop_known_values() {
        let (_d, e) = engine();
        e.present(
            &file(".env"),
            format!("PAYMENTS_API_KEY={KEY}\n").as_bytes(),
        );
        let (filter, check) = e.outbound();
        let mut req = Request {
            items: vec![
                Item::User {
                    text: format!("use key {KEY} and email {EMAIL}"),
                },
                Item::ToolResult {
                    call_id: "c".into(),
                    content: format!("value {KEY}"),
                },
            ],
            ..Request::default()
        };
        let notes = filter.apply(&mut req);
        assert!(!notes.is_empty());
        let body = serde_json::to_value(&req.items).unwrap();
        assert!(!body.to_string().contains(KEY) && !body.to_string().contains(EMAIL));
        assert!(check.check(&body).is_ok());
        assert!(check.check(&json!({"x": format!("oops {KEY}")})).is_err());
    }

    #[test]
    fn search_hides_matches_in_sensitive_files() {
        let (_d, e) = engine();
        let text = format!("src/lib.rs:3: let x = 1;\ndata/customers.csv:2: C-1,{EMAIL},EUR\n");
        let shown = e.present(
            &Source::Search {
                pattern: "x".into(),
            },
            text.as_bytes(),
        );
        assert!(shown.contains("src/lib.rs:3: let x = 1;"));
        assert!(shown.contains("data/customers.csv:2: [match in sensitive content"));
        assert!(!shown.contains(EMAIL));
    }

    #[test]
    fn repeated_values_keep_one_token_across_sources() {
        let (_d, e) = engine();
        let a = e.present(
            &file("logs/a.log"),
            format!("ERROR user {EMAIL} failed\n").as_bytes(),
        );
        let b = e.present(
            &file("src/x.rs"),
            format!("// contact {EMAIL}\n").as_bytes(),
        );
        let token = "⟨email:email#1⟩";
        assert!(a.contains(token) && b.contains(token), "{a}\n{b}");
    }
}

#[cfg(test)]
mod prime_tests {
    use super::*;

    #[test]
    fn primed_values_are_replaced_in_any_later_output() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::write(ws.join(".env"), "API_TOKEN=Qx7pL2mN9vR4tY8wZ3kD\n").unwrap();
        std::fs::write(
            ws.join("data/customers.csv"),
            "id,name,email,balance\n1,Priya Tolvenrin,priya.t@mailbox-9.net,4812339\n",
        )
        .unwrap();
        let policy = Policy {
            sensitive_globs: vec![".env*".into(), "data/**".into()],
            command_output_sensitive: true,
            detect_secrets: true,
            detect_pii: true,
            detect_entropy: true,
            ..Policy::default()
        };
        let e = Engine::open(&d.path().join("run"), policy, None).unwrap();
        let files = [
            ".env".to_string(),
            "data/customers.csv".to_string(),
            "src/lib.rs".to_string(),
        ];
        assert_eq!(e.prime(&ws, &files, ""), 2);
        let out = "API_TOKEN=Qx7pL2mN9vR4tY8wZ3kD\n1,Priya Tolvenrin,priya.t@mailbox-9.net,4812339\ntest result: ok. 1772361000\n";
        let src = Source::Command {
            command: "cat .env data/customers.csv".into(),
            exit_code: Some(0),
        };
        let shown = e.present(&src, out.as_bytes());
        for secret in [
            "Qx7pL2mN9vR4tY8wZ3kD",
            "Priya Tolvenrin",
            "priya.t@mailbox-9.net",
            "4812339",
        ] {
            assert!(!shown.contains(secret), "{secret} leaked: {shown}");
        }
        assert!(
            shown.contains("1772361000"),
            "ordinary numbers stay visible: {shown}"
        );
    }

    #[test]
    fn the_models_own_messages_are_sanitized_too() {
        // The model can rebuild a value it never saw verbatim (from character
        // codes, a reformatted number); its history must not carry it out.
        let d = tempfile::tempdir().unwrap();
        let policy = Policy {
            detect_secrets: true,
            sensitive_globs: vec![".env*".into()],
            ..Policy::default()
        };
        let e = Engine::open(d.path(), policy, None).unwrap();
        e.present(
            &Source::File {
                path: ".env".into(),
                ranged: false,
            },
            b"API_TOKEN=Qx7pL2mN9vR4tY8wZ3kD\n",
        );
        let (filter, check) = e.outbound();
        let own =
            json!({"messages": [{"role": "assistant", "content": "I guess Qx7pL2mN9vR4tY8wZ3kD"}]});
        assert!(check.check(&own).is_err());
        let mut req = Request {
            items: vec![Item::Assistant {
                text: "decoded: Qx7pL2mN9vR4tY8wZ3kD".into(),
                reasoning: Some("the codes spell Qx7pL2mN9vR4tY8wZ3kD".into()),
                tool_calls: vec![crate::model::ToolCall {
                    id: "c1".into(),
                    name: "write_file".into(),
                    arguments: Map::new(),
                    raw_arguments: r#"{"path":"a.txt","content":"Qx7pL2mN9vR4tY8wZ3kD"}"#.into(),
                }],
            }],
            ..Request::default()
        };
        assert!(!filter.apply(&mut req).is_empty());
        let body = serde_json::to_value(&req.items).unwrap();
        assert!(!body.to_string().contains("Qx7pL2mN9vR4tY8wZ3kD"), "{body}");
        assert!(check.check(&body).is_ok());
        let Item::Assistant { tool_calls, .. } = &req.items[0] else {
            unreachable!()
        };
        assert_eq!(
            tool_calls[0].arguments["content"], "⟨secret:API_TOKEN#1⟩",
            "parsed arguments follow the sanitized raw arguments"
        );
    }

    fn primed_engine(files: &[(&str, &str)], objective: &str) -> (tempfile::TempDir, Arc<Engine>) {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        for (path, text) in files {
            let p = ws.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let policy = Policy {
            sensitive_globs: vec!["data/**".into()],
            command_output_sensitive: true,
            detect_pii: true,
            ..Policy::default()
        };
        let e = Engine::open(&d.path().join("run"), policy, None).unwrap();
        let names: Vec<String> = files.iter().map(|(p, _)| p.to_string()).collect();
        e.prime(&ws, &names, objective);
        (d, e)
    }

    #[test]
    fn surnames_are_replaced_alone_unless_the_word_is_public() {
        let (_d, e) = primed_engine(
            &[
                (
                    "data/bank.csv",
                    "Buchungsdatum;Verwendungszweck\n03.08.2026;Miete August Jonas Zetharsko\n04.08.2026;Error Priya Tolvenrin\n",
                ),
                ("src/lib.rs", "// parses bank statements\n"),
            ],
            "Fix the August month-end reconciliation.",
        );
        let shown = e.present(
            &Source::File {
                path: "src/notes.rs".into(),
                ranged: false,
            },
            b"// Zetharsko paid; Tolvenrin too; closes in August\n",
        );
        assert!(
            !shown.contains("Zetharsko") && !shown.contains("Tolvenrin"),
            "{shown}"
        );
        assert!(shown.contains("August"), "public words stay: {shown}");
    }

    #[test]
    fn the_task_names_the_sensitive_paths() {
        let (_d, e) = primed_engine(
            &[
                ("data/customers.csv", "id\n1\n"),
                ("src/lib.rs", "// code\n"),
            ],
            "",
        );
        let task = e.sanitize_objective("Fix the export.");
        assert!(task.starts_with("Fix the export."), "{task}");
        assert!(
            task.contains("data/customers.csv") && !task.contains("src/lib.rs"),
            "{task}"
        );
        assert!(task.contains("sensitive_data"), "{task}");
    }

    #[test]
    fn numbers_are_replaced_in_their_other_spellings() {
        let (_d, e) = primed_engine(
            &[(
                "data/customers.csv",
                "id,currency,balance_minor\nC-1066,JPY,\"2,038,382\"\nC-1001,EUR,5186126\n",
            )],
            "",
        );
        let out =
            "C-1066 2038382\nC-1001 51861.26 51,861.26 5,186,126\ntests: 12 passed in 0.31s\n";
        let shown = e.present(
            &Source::Command {
                command: "cargo run".into(),
                exit_code: Some(0),
            },
            out.as_bytes(),
        );
        for v in ["2038382", "51861.26", "51,861.26", "5,186,126"] {
            assert!(!shown.contains(v), "{v} leaked: {shown}");
        }
        assert!(shown.contains("12 passed in 0.31s"), "{shown}");
    }
}
