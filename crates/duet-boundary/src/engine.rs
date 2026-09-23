// SPDX-License-Identifier: GPL-3.0-or-later
//! The security engine: decides what the frontier sees of every tool result,
//! and sanitizes every outbound request.
//!
//! - Public content: shown, with detected secrets/PII replaced by placeholders.
//! - Secret-bearing files (`.env`-style): shown with every value replaced.
//! - Sensitive data, logs and command output: stored under a handle; the
//!   frontier gets error lines (sanitized) and a local-model summary, and can
//!   ask the local model questions.
//! - Writes: placeholders are resolved locally, and secrets may only land in
//!   secret files.

use crate::detect::{Detectors, Kind, scan};
use crate::gate::{OutboundCheck, OutboundFilter};
use crate::handles::HandleStore;
use crate::local::LocalReader;
use crate::model::{Item, Request, ToolSpec};
use crate::overlap::OverlapIndex;
use crate::policy::{Policy, is_secret_bearing};
use crate::vault::Vault;
use crate::view::{Presenter, Source};
use duet_fs::FsError;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

static NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\p{Lu}\p{Ll}+(?:[ '-]\p{Lu}\p{Ll}+){1,2}\b").expect("static regex")
});
static LONG_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{1,3}(?:,\d{3}){2,}\b|\b\d{6,}\b").expect("static regex"));
static ERROR_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(error|panic|panicked|exception|traceback|failed|failure|fatal|warn|warning|invalid|unwrap|denied|timeout)\b")
        .expect("static regex")
});
/// Words that look like names in title case but are not personal data.
const NAME_STOPWORDS: &[&str] = &[
    "Result", "Option", "Error", "String", "Vec", "Some", "None", "Ok", "Err", "Self", "Warn",
    "Info", "Debug",
];

struct State {
    vault: Vault,
    handles: HandleStore,
    overlap: OverlapIndex,
}

pub struct Engine {
    policy: Policy,
    detectors: Detectors,
    local: Option<LocalReader>,
    state: Mutex<State>,
}

pub const MAX_KEY_LINES: usize = 12;

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
            state: Mutex::new(State {
                vault: Vault::open(&run_dir.join("vault.json"))?,
                handles: HandleStore::open(&run_dir.join("handles"))?,
                overlap: OverlapIndex::default(),
            }),
        }))
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
                if !NAME_STOPWORDS
                    .iter()
                    .any(|w| m.as_str().split(' ').any(|p| p == *w))
                {
                    spans.push((m.start(), m.end(), Kind::Name, None));
                }
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
    fn handle_view(&self, source_label: &str, text: &str, public: bool) -> String {
        let handle = {
            let mut st = self.lock();
            st.overlap.add_sensitive(text);
            match st.handles.put(text.as_bytes(), source_label, public) {
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

    /// Local-model output: sanitized as sensitive text, then copied spans removed.
    fn clean_local(&self, st: &mut State, text: &str, origin: &str) -> String {
        let s = self.sanitize(st, text, origin, true);
        st.overlap.redact(&s).0
    }

    fn search_view(&self, text: &str) -> String {
        let mut st = self.lock();
        let mut out = String::new();
        for line in text.lines() {
            let path = line.split(':').next().unwrap_or_default();
            if !path.is_empty() && self.policy.is_sensitive_path(Path::new(path)) {
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
        let question = args
            .get("question")
            .and_then(Value::as_str)
            .ok_or("missing `question`")?;
        let (info, bytes) = self
            .lock()
            .handles
            .get(id)
            .ok_or_else(|| format!("unknown handle {id}"))?;
        let local = self.local.as_ref().ok_or("no local model is configured")?;
        let q = self.detokenize(question);
        let text = String::from_utf8_lossy(&bytes);
        let a = Self::block_on(local.answer(&info.source, &text, &q))
            .map_err(|e| format!("local model: {}", e.message))?;
        let mut st = self.lock();
        let answer = self.clean_local(&mut st, &a.answer, &info.source);
        Ok(if a.unanswerable {
            format!("The local model could not answer from {id}. {answer}")
        } else {
            format!("{answer}\n(evidence lines: {:?})", a.evidence_lines)
        })
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
        let start = args
            .get("start_line")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1) as usize;
        let end = args
            .get("end_line")
            .and_then(Value::as_u64)
            .map_or(start + 199, |e| e as usize);
        let chunk: String = text
            .lines()
            .enumerate()
            .skip(start - 1)
            .take(end.saturating_sub(start) + 1)
            .map(|(i, l)| format!("{:>6}  {l}\n", i + 1))
            .collect();
        let mut st = self.lock();
        Ok(self.sanitize(&mut st, &chunk, &info.source, false))
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
        match source {
            Source::File { path } if self.policy.is_sensitive_path(path) => {
                let label = path.display().to_string();
                if is_secret_bearing(path) {
                    let mut st = self.lock();
                    st.overlap.add_sensitive(&text);
                    self.tokenized_view(&mut st, &label, &text)
                } else {
                    self.handle_view(&label, &text, false)
                }
            }
            Source::File { path } => {
                let mut st = self.lock();
                st.overlap.add_public(&text);
                self.sanitize(&mut st, &text, &path.display().to_string(), false)
            }
            Source::Command { command, .. } | Source::Other { label: command }
                if self.policy.command_output_sensitive
                    && !self.policy.command_is_raw_ok(command) =>
            {
                self.handle_view(&format!("output of `{command}`"), &text, false)
            }
            Source::Checks if self.policy.command_output_sensitive => {
                self.handle_view("check output", &text, false)
            }
            Source::Search { .. } => self.search_view(&text),
            _ => {
                let mut st = self.lock();
                self.sanitize(&mut st, &text, "tool output", false)
            }
        }
    }

    fn extra_tools(&self) -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: "ask_local".into(),
                description: "Ask the local model a question about content held under a handle (sensitive files, logs, \
command output). It reads the raw content on this machine and answers without revealing sensitive values."
                    .into(),
                parameters: json!({"type": "object", "properties": {
                    "handle": {"type": "string", "description": "A handle such as h3."},
                    "question": {"type": "string"}
                }, "required": ["handle", "question"]}),
            },
            ToolSpec {
                name: "read_raw".into(),
                description: "Read a line range of a large public result held under a handle.".into(),
                parameters: json!({"type": "object", "properties": {
                    "handle": {"type": "string"},
                    "start_line": {"type": "integer", "minimum": 1},
                    "end_line": {"type": "integer", "minimum": 1}
                }, "required": ["handle"]}),
            },
        ]
    }

    fn call_tool(&self, name: &str, args: &Map<String, Value>) -> Option<Result<String, String>> {
        match name {
            "ask_local" => Some(self.ask_local(args)),
            "read_raw" => Some(self.read_raw(args)),
            _ => None,
        }
    }

    fn detokenize(&self, text: &str) -> String {
        self.lock().vault.detokenize(text).0
    }

    fn resolve_for_write(&self, path: &Path, text: &str) -> Result<String, String> {
        let st = self.lock();
        let sink = self.policy.is_secret_sink(path) || self.policy.is_sensitive_path(path);
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

    fn sanitize_objective(&self, text: &str) -> String {
        let mut st = self.lock();
        self.sanitize(&mut st, text, "task", false)
    }
}

/// Outbound filter: sanitizes every user item and tool result again (idempotent).
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
                Item::Assistant { .. } => continue,
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
            shown.contains("ask_local") && !shown.contains(EMAIL),
            "{shown}"
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
