// SPDX-License-Identifier: GPL-3.0-or-later
//! Structure views, synthetic samples, masked output and the budget of
//! short outputs of commands that read sensitive data, over
//! [`crate::structure`] with the engine's vault and vocabulary.
//!
//! - A sensitive file's view gets its structure view (replacing the map of
//!   repeated line shapes) and, for a data file, the first record of its
//!   synthetic sample; the task note an outline of every file the policy
//!   makes sensitive.
//! - `synthetic_sample(handle, rows)` makes a sample of a sensitive file:
//!   a fake is a function of its value's shape and first position and the
//!   run's seed, never of the value. Before it is shown, no known value may
//!   be in it and no span of it may be copied from sensitive content;
//!   otherwise it is withheld (fail-closed). Its detected fakes (card
//!   numbers, phone numbers) are noted as the frontier's own, so the gate
//!   shows them as written; a file written with nothing but its lines is a
//!   fixture, not sensitive data, while its content stays that.
//! - A short output of a `sensitive_data` command (or of a file such a
//!   command wrote) is a probe: counted against `sensitivity.output_probes`
//!   and audited; past the budget the view is a fixed text that does not
//!   depend on the output. Output that is shown comes masked: every value as
//!   its shape, small numbers within `sensitivity.masked_numbers`.

use super::{Engine, State};
use crate::audit::AuditEvent;
use crate::detect::Kind;
use crate::structure::twin::{Twin, mix};
use crate::structure::{self, Knowledge, masked, profile, twin};
use aho_corasick::{AhoCorasick, MatchKind};
use regex::Regex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Program output of at most this many characters is short: a count, a
/// match, a yes or no.
const SHORT_OUTPUT_CHARS: usize = 200;
/// Masked output is shown for output up to this many lines (and
/// [`super::INLINE_OUTPUT_CHARS`] characters).
const MASKED_LINES: usize = 80;
/// The first record of a sample goes with a file's view when at most this long.
const INLINE_SAMPLE_CHARS: usize = 2500;
/// Records a sample holds when the call names none.
const DEFAULT_ROWS: usize = 5;
/// Characters of the structure outline in the task note, per file and in all.
const OUTLINE_FILE_CHARS: usize = 1200;
const OUTLINE_CHARS: usize = 4000;
/// Seeds tried for a sample that passes the checks.
const SAMPLE_ATTEMPTS: u64 = 3;
/// Values of structured sensitive data remembered as values (the rest are
/// still in the vault when a detector found them).
const DATA_VALUES_MAX: usize = 200_000;
/// A value this short is remembered as a word only (a code, a category).
const DATA_VALUE_MIN: usize = 4;

/// JSON's literals: syntax, not values, in a sample's check.
static LITERAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:true|false|null)\b").expect("static regex"));

/// Where content under a handle came from.
#[derive(Debug, Clone, Copy)]
pub(super) enum Origin<'a> {
    File(&'a Path),
    /// A command that read sensitive data (its command line).
    Command(&'a str),
    Other,
}

/// What the structure views know, per run.
#[derive(Default)]
pub(super) struct StructureState {
    /// One-word values of structured sensitive data, lower-cased (`settled`).
    data_words: HashSet<String>,
    /// Values of structured sensitive data (4+ characters), and a matcher
    /// over them built on first use after a change.
    data_values: std::collections::BTreeSet<String>,
    data_matcher: std::sync::OnceLock<Option<AhoCorasick>>,
    /// Words of the schema names shown (keys, columns).
    schema_words: HashSet<String>,
    /// The seed of this run's samples (persisted in `sample-seed`).
    seed: u64,
    /// Hashes of every line of every sample shown.
    sample_lines: HashSet<u64>,
    /// Outline of each sensitive file, by path, for the task note.
    outline: BTreeMap<String, String>,
}

/// Files written with sample lines only: path → SHA-256 of that content
/// (persisted in `fixtures.json`), and the workspace they are in; and the
/// files a command that read sensitive data wrote (persisted in
/// `rewritten.json`), whose content that command chose.
#[derive(Default)]
pub(super) struct Fixtures {
    files: BTreeMap<PathBuf, String>,
    workspace: Option<PathBuf>,
    rewritten: std::collections::BTreeSet<PathBuf>,
}

fn line_hash(line: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    line.trim().hash(&mut h);
    h.finish()
}

fn random_seed() -> u64 {
    use std::hash::BuildHasher;
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    std::time::SystemTime::now().hash(&mut h);
    std::process::id().hash(&mut h);
    h.finish()
}

impl StructureState {
    /// The run's state: its sample seed read back, or drawn and kept.
    pub(super) fn open(run_dir: &Path) -> Self {
        let file = run_dir.join("sample-seed");
        let seed = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or_else(|| {
                let s = random_seed();
                let _ = duet_fs::private::write_private(&file, s.to_string().as_bytes());
                s
            });
        Self {
            seed,
            ..Self::default()
        }
    }
}

impl Fixtures {
    pub(super) fn open(run_dir: &Path) -> Self {
        let read = |name: &str| std::fs::read(run_dir.join(name)).ok();
        Self {
            files: read("fixtures.json")
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default(),
            workspace: None,
            rewritten: read("rewritten.json")
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default(),
        }
    }
}

/// What the engine knows: its vault, the public and schema words, and the
/// words of values in sensitive data.
pub(super) struct Known<'a>(pub(super) &'a State);

impl StructureState {
    fn data_matcher(&self) -> Option<&AhoCorasick> {
        self.data_matcher
            .get_or_init(|| {
                (!self.data_values.is_empty())
                    .then(|| {
                        AhoCorasick::builder()
                            .match_kind(MatchKind::LeftmostLongest)
                            .ascii_case_insensitive(true)
                            .build(&self.data_values)
                            .ok()
                    })
                    .flatten()
            })
            .as_ref()
    }

    /// Remembers the values of structured sensitive content.
    fn note_values(&mut self, values: Vec<String>) {
        let before = self.data_values.len();
        for v in values {
            if !v.chars().any(char::is_alphanumeric) || structure::marker_word(&v) {
                continue;
            }
            if v.chars().all(|c| c.is_alphanumeric() || c == '_') {
                self.data_words.insert(v.to_lowercase());
            }
            if v.chars().count() >= DATA_VALUE_MIN && self.data_values.len() < DATA_VALUES_MAX {
                self.data_values.insert(v);
            }
        }
        if self.data_values.len() != before {
            self.data_matcher = std::sync::OnceLock::new();
        }
    }
}

impl Knowledge for Known<'_> {
    /// Known values (the vault), and values of structured sensitive data
    /// wherever they occur (as `data`).
    fn values(&self, text: &str) -> Vec<(usize, usize, Kind)> {
        let mut out = self.0.vault.spans(text);
        if let Some(ac) = self.0.structure.data_matcher() {
            let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            for m in ac.find_iter(text) {
                // Whole words only (`Novapay` is not in `NOVAPAY_API_KEY`),
                // and a value inside a known one is already covered.
                if word(text[..m.start()].chars().next_back())
                    || word(text[m.end()..].chars().next())
                    || out.iter().any(|&(s, e, _)| m.start() < e && s < m.end())
                {
                    continue;
                }
                out.push((m.start(), m.end(), Kind::Data));
            }
            out.sort_unstable_by_key(|x| x.0);
        }
        out
    }
    fn public_word(&self, word: &str) -> bool {
        self.0.public_words.contains(word) || self.0.structure.schema_words.contains(word)
    }
    fn data_word(&self, word: &str) -> bool {
        self.0.structure.data_words.contains(word)
    }
    /// A known value, or a value of structured sensitive data: what no
    /// fake of a sample may hold.
    fn known(&self, text: &str) -> bool {
        !self.0.vault.values_in(text).is_empty()
            || self
                .0
                .structure
                .data_matcher()
                .is_some_and(|ac| ac.is_match(text))
    }
}

/// The program's part of a command's output: without Duet's framing lines.
fn program_output(text: &str) -> String {
    text.lines()
        .filter(|l| !masked::framing(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a command's output is short: a count, a match, a yes or no.
fn short_output(text: &str) -> bool {
    program_output(text).trim().chars().count() <= SHORT_OUTPUT_CHARS
}

impl Engine {
    fn run_dir(&self) -> &Path {
        self.probes_file.parent().unwrap_or(Path::new("."))
    }

    fn fixtures(&self) -> std::sync::MutexGuard<'_, Fixtures> {
        self.fixtures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Notes the workspace the engine was primed on (fixtures are read there).
    pub(super) fn structure_prime(&self, workspace: &Path) {
        self.fixtures().workspace = Some(workspace.to_path_buf());
    }

    /// Whether `path` is a fixture written from samples whose content is
    /// still exactly what was checked.
    pub(super) fn is_fixture(&self, path: &Path) -> bool {
        let f = self.fixtures();
        let (Some(digest), Some(ws)) = (f.files.get(path), f.workspace.as_ref()) else {
            return false;
        };
        duet_fs::read_file(ws, path, super::PRIME_MAX_BYTES as u64)
            .is_ok_and(|bytes| hex::encode(Sha256::digest(&bytes)) == *digest)
    }

    /// Before a write of `content` to `path`: a file matching the
    /// sensitivity globs whose every line comes from a sample shown in
    /// this run is a fixture (not sensitive) for as long as it holds
    /// exactly this content; any other content ends that.
    pub(super) fn note_fixture(&self, path: &Path, content: &str) {
        let all_sample = {
            let st = self.lock();
            let mut lines = content.lines().filter(|l| !l.trim().is_empty()).peekable();
            lines.peek().is_some()
                && lines.all(|l| st.structure.sample_lines.contains(&line_hash(l)))
        };
        let p = path.to_string_lossy();
        let glob = self
            .policy
            .sensitive_globs
            .iter()
            .any(|g| crate::policy::glob_match(g, &p))
            && !self
                .policy
                .protected_paths
                .iter()
                .any(|g| crate::policy::glob_match(g, &p));
        let derived = self
            .derived
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(path);
        let mut f = self.fixtures();
        let changed = if all_sample && glob && !derived {
            let digest = hex::encode(Sha256::digest(content.as_bytes()));
            f.files.insert(path.to_path_buf(), digest.clone()) != Some(digest)
        } else {
            f.files.remove(path).is_some()
        };
        if changed {
            let _ = duet_fs::private::write_private(
                &self.run_dir().join("fixtures.json"),
                &serde_json::to_vec(&f.files).unwrap_or_default(),
            );
        }
    }

    /// Whether `path` is sensitive by policy and not written by a command
    /// that read sensitive data: content the frontier did not choose, of
    /// which outlines and samples may be made.
    fn primary(&self, path: &Path) -> bool {
        self.policy.is_sensitive_path(path) && !self.fixtures().rewritten.contains(path)
    }

    /// Files a command that read sensitive data wrote: from now on their
    /// content is the command's, not the data's.
    pub(super) fn note_rewritten(&self, paths: &[PathBuf]) {
        let mut f = self.fixtures();
        let before = f.rewritten.len();
        f.rewritten.extend(paths.iter().cloned());
        if f.rewritten.len() != before {
            let _ = duet_fs::private::write_private(
                &self.run_dir().join("rewritten.json"),
                &serde_json::to_vec(&f.rewritten).unwrap_or_default(),
            );
        }
    }

    /// Indexes a sensitive file for the structure views: the values it
    /// holds, and (a file the policy makes sensitive) its outline for the
    /// task note.
    pub(super) fn index_structure(&self, st: &mut State, path: &Path, text: &str) {
        if !self.policy.structure.views {
            return;
        }
        st.structure
            .note_values(structure::values(text, Some(path)));
        if !self.primary(path) {
            st.structure.outline.remove(&path.display().to_string());
            return;
        }
        let outline =
            profile::profile(text, Some(path), &Known(st)).map(|p| p.outline(OUTLINE_FILE_CHARS));
        if let Some(o) = outline {
            st.structure.outline.insert(path.display().to_string(), o);
        }
    }

    /// The task note's outline of the sensitive files.
    pub(super) fn outline_note(st: &State) -> String {
        if st.structure.outline.is_empty() {
            return String::new();
        }
        let mut out = String::from(
            "\n\nStructure of the sensitive files (read here without a model; counts, shapes and layouts, \
never values; read_file shows each in full):",
        );
        let mut left = OUTLINE_CHARS;
        for (path, o) in &st.structure.outline {
            let line = format!("\n- {path}: {o}");
            if line.len() > left {
                out.push_str("\n- …");
                break;
            }
            left -= line.len();
            out.push_str(&line);
        }
        out
    }

    /// A short output of a command that read sensitive data, or of a file
    /// such a command wrote, is a probe: counted and audited. `Some` holds
    /// the fixed view that replaces it once the run's budget is spent.
    pub(super) fn probe_gate(&self, text: &str, origin: Origin<'_>) -> Option<String> {
        let budget = self.policy.structure.output_probes?;
        let what = match origin {
            Origin::Command(command) => format!("Output of `{command}`"),
            Origin::File(path)
                if self.fixtures().rewritten.contains(path)
                    || self
                        .derived
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .contains(path) =>
            {
                format!(
                    "{} (written by a command that read sensitive data)",
                    path.display()
                )
            }
            _ => return None,
        };
        if !short_output(text) {
            return None;
        }
        let (count, shown) = {
            let mut st = self.lock();
            let r = st.probes.output_probe(budget);
            let _ = duet_fs::private::write_private(
                &self.probes_file,
                &serde_json::to_vec(&st.probes).unwrap_or_default(),
            );
            r
        };
        self.record(AuditEvent::OutputProbe {
            count,
            budget,
            shown,
        });
        (!shown).then(|| {
            format!(
                "{what} withheld. A short output derived from sensitive data (a count, a match, a yes or \
no) is a probe of it, and this run's {budget} are spent (sensitivity.output_probes); nothing about it is \
shown, not its size and not its exit code. The structure views of the sensitive files (read_file) and \
synthetic_sample tell their layout, formats and counts without a probe.\n"
            )
        })
    }

    /// Output of a command that read sensitive data, every value masked, if
    /// it is short enough to show whole.
    pub(super) fn masked_section(
        &self,
        st: &mut State,
        handle: &str,
        label: &str,
        text: &str,
        command: &str,
    ) -> Option<String> {
        if !self.policy.structure.views
            || text.len() > super::INLINE_OUTPUT_CHARS
            || text.lines().count() > MASKED_LINES
        {
            return None;
        }
        // Every value the output holds is known before it is masked.
        let _ = self.sanitize(st, text, label, true);
        let budget = self.policy.structure.masked_numbers;
        let mut numbers = masked::Budget {
            left: st.probes.numbers_left(budget),
            shown: 0,
            masked: 0,
        };
        let out = masked::mask(text, command, &Known(st), &mut numbers);
        if numbers.shown > 0 {
            st.probes.numbers_shown(numbers.shown);
            let _ = duet_fs::private::write_private(
                &self.probes_file,
                &serde_json::to_vec(&st.probes).unwrap_or_default(),
            );
            self.record(AuditEvent::MaskedNumbers {
                handle: handle.to_owned(),
                shown: numbers.shown,
                left: st.probes.numbers_left(budget),
            });
        }
        let out: String = out.lines().map(|l| format!("  {l}\n")).collect();
        Some(format!(
            "Output with every value masked (letters as A/a, digits as 9; words of the public files and of \
the command kept; numbers 0-99 as written while sensitivity.masked_numbers lasts):\n{out}"
        ))
    }

    /// The structure view of `text`, if it has a structure worth telling.
    /// The schema names it shows become words masked output may keep.
    pub(super) fn structure_section(
        &self,
        st: &mut State,
        text: &str,
        origin: Origin<'_>,
    ) -> Option<String> {
        if !self.policy.structure.views {
            return None;
        }
        let path = match origin {
            Origin::File(p) => Some(p),
            _ => None,
        };
        let p = profile::profile(text, path, &Known(st))?;
        for name in &p.schema {
            st.structure.schema_words.extend(
                name.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .filter(|w| !w.is_empty())
                    .map(str::to_lowercase),
            );
            st.structure.schema_words.insert(name.to_lowercase());
        }
        Some(p.render())
    }

    /// A sample of a policy-sensitive file, checked: no known value in it
    /// (JSON's literals aside), no span copied from sensitive content,
    /// every value faked. Shown samples register their lines (fixtures),
    /// their verbatim schema lines as public text and their detected fakes
    /// as the frontier's own.
    pub(super) fn sample(
        &self,
        st: &mut State,
        label: &str,
        text: &str,
        path: Option<&Path>,
        rows: usize,
    ) -> Result<Twin, String> {
        // Command output and the like may hold values priming did not see.
        let _ = self.sanitize(st, text, label, true);
        let mut last = String::new();
        for attempt in 0..SAMPLE_ATTEMPTS {
            let t = twin::twin(
                text,
                path,
                &Known(st),
                mix(st.structure.seed, attempt),
                rows,
            )?;
            for v in &t.verbatim {
                st.overlap.add_public(v);
            }
            let body = LITERAL.replace_all(&t.text, " ");
            last = if t.stuck > 0 || !st.vault.values_in(&body).is_empty() {
                "a known value".into()
            } else if st.overlap.redact(&t.text).1 > 0 {
                "a span copied from sensitive content".into()
            } else {
                for l in t.text.lines().filter(|l| !l.trim().is_empty()) {
                    st.structure.sample_lines.insert(line_hash(l));
                }
                for f in crate::detect::scan_with(&t.text, self.detectors, &self.custom) {
                    let value = &t.text[f.start..f.end];
                    if !st.vault.contains(value) {
                        st.authored.insert(value.to_owned());
                    }
                }
                return Ok(t);
            };
        }
        Err(format!(
            "withheld: a check found {last} in every sample drawn (nothing was shown)"
        ))
    }

    /// The first record of a data file's sample, for its view.
    pub(super) fn inline_sample(
        &self,
        st: &mut State,
        handle: &str,
        label: &str,
        text: &str,
        path: &Path,
    ) -> Option<String> {
        let rows = self.policy.structure.synthetic_rows;
        if !self.policy.structure.views
            || rows == 0
            || !self.primary(path)
            || !structure::detect(text, Some(path)).has_records()
        {
            return None;
        }
        let more = format!(
            "synthetic_sample(handle=\"{handle}\", rows=…) gives up to {rows} records as a test fixture"
        );
        match self.sample(st, label, text, Some(path), 1) {
            Ok(t) if t.text.len() <= INLINE_SAMPLE_CHARS => {
                self.record(AuditEvent::SyntheticSample {
                    handle: handle.to_owned(),
                    records: t.records.len() as u32,
                    outcome: "shown".into(),
                });
                Some(format!(
                    "Synthetic sample (record {} of {}; every value generated from its shape, not real data, \
not sensitive; {more}):\n{}\n",
                    t.records[0],
                    t.total,
                    t.text.trim_end()
                ))
            }
            Ok(_) => Some(format!("[{more}]\n")),
            Err(e) if e.starts_with("withheld") => {
                self.record(AuditEvent::SyntheticSample {
                    handle: handle.to_owned(),
                    records: 0,
                    outcome: "withheld".into(),
                });
                Some(format!("[synthetic sample {e}]\n"))
            }
            Err(_) => None,
        }
    }

    /// `synthetic_sample {handle, rows?}`.
    pub(super) fn synthetic_sample(&self, args: &Map<String, Value>) -> Result<String, String> {
        let id = args
            .get("handle")
            .and_then(Value::as_str)
            .ok_or("missing `handle`")?;
        let max = self.policy.structure.synthetic_rows;
        let rows = args
            .get("rows")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_ROWS, |r| r as usize)
            .clamp(1, max.max(1));
        let (info, bytes) = self
            .lock()
            .handles
            .get(id)
            .ok_or_else(|| format!("unknown handle {id}"))?;
        if info.public {
            return Err(format!("{id} holds public content: read it with read_raw"));
        }
        let path = super::origin_path(&info.source).map(Path::new);
        let sensitive_file = path.is_some_and(|p| self.primary(p));
        if !sensitive_file {
            return Err(format!(
                "{id} is not a sensitive file ({}): samples are made of the data files themselves, not of \
command output or files commands wrote",
                info.source
            ));
        }
        let text = String::from_utf8_lossy(&bytes);
        // A file's handle holds it as shown: numbered.
        let text = crate::bulky::strip_line_numbers(&text)
            .map_or_else(|| text.to_string(), |(_, body)| body);
        let mut st = self.lock();
        let outcome = self.sample(&mut st, &info.source, &text, path, rows);
        let (records, word, result) = match outcome {
            Ok(t) => (
                t.records.len() as u32,
                "shown",
                Ok(format!(
                    "Synthetic sample of {} ({}): records {} of {}, every value generated from its shape \
(same formats, lengths, nulls and quoting; dates valid; card numbers and IBANs pass their checks; emails at \
.test). Not real data and not sensitive: use it as a test fixture. A file written with nothing but these \
lines stays readable to commands even under a sensitivity glob (such as *.csv) until something else changes \
it.\n{}",
                    info.source,
                    t.format.name(),
                    t.records
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                    t.total,
                    t.text
                )),
            ),
            Err(e) if e.starts_with("withheld") => (
                0,
                "withheld",
                Err(format!(
                    "synthetic sample of {id} {e}; its structure view is what can be shown"
                )),
            ),
            Err(e) => (
                0,
                "unsupported",
                Err(format!("no synthetic sample of {id}: {e}")),
            ),
        };
        drop(st);
        self.record(AuditEvent::SyntheticSample {
            handle: id.to_owned(),
            records,
            outcome: word.into(),
        });
        result
    }

    /// The `.env`-style view's addition: how each value is written.
    pub(super) fn env_formats(&self, st: &mut State, path: &Path, text: &str) -> String {
        if !self.policy.structure.views {
            return String::new();
        }
        match profile::profile(text, Some(path), &Known(st)) {
            Some(p) if p.format == structure::Format::Env => {
                for name in &p.schema {
                    st.structure.schema_words.insert(name.to_lowercase());
                }
                p.render()
            }
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::policy;
    use super::*;
    use crate::policy::{Policy, StructureSettings};
    use crate::view::{Presenter, Source};
    use std::path::PathBuf;
    use std::sync::Arc;

    const NAMES: [&str; 3] = ["Vakdril Thorsko", "Orla Brennvik", "Ysolde Marrquin"];
    const EMAILS: [&str; 3] = [
        "vakdril.thorsko@kestrelpost-mail.net",
        "orla.brennvik@fjordmail-post.org",
        "ysolde.marrquin@quillhaven-mail.io",
    ];
    const CARDS: [&str; 3] = ["4539148803436467", "5293761582049377", "6011000990139424"];
    const BORN: [&str; 3] = ["1987-03-14", "1994-11-07", "1979-06-21"];

    fn views(probes: Option<u32>) -> Policy {
        let mut p = Policy {
            structure: StructureSettings {
                views: true,
                synthetic_rows: 20,
                masked_numbers: 3,
                output_probes: probes,
            },
            ..policy()
        };
        p.sensitive_globs.push("*.csv".into());
        p
    }

    fn csv() -> String {
        let mut csv = String::from("id,name,born,email,card,status\n");
        for i in 0..3 {
            csv.push_str(&format!(
                "{},{},{},{},{},{}\n",
                i + 1,
                NAMES[i],
                BORN[i],
                EMAILS[i],
                CARDS[i],
                ["settled", "pending", "settled"][i]
            ));
        }
        csv
    }

    /// A workspace with `data/customers.csv` and a public source file, the
    /// engine primed on it with a fixed sample seed.
    fn primed(policy: Policy) -> (tempfile::TempDir, std::path::PathBuf, Arc<Engine>) {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("ws");
        let run = d.path().join("run");
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(ws.join("data/customers.csv"), csv()).unwrap();
        std::fs::write(
            ws.join("src/lib.rs"),
            "// Parses the customer export rows (born date, card, status: settled or pending);\n\
             // a row that failed is reported at its line, column and offset.\nfn main() {}\n",
        )
        .unwrap();
        std::fs::write(run.join("sample-seed"), "7").unwrap();
        let e = Engine::open(&run, policy, None).unwrap();
        e.prime(
            &ws,
            &["data/customers.csv".into(), "src/lib.rs".into()],
            "Fix the export of customers.",
        );
        (d, ws, e)
    }

    /// `text` presented as `read_file` presents a file: numbered.
    fn read(e: &Engine, path: &str, text: &str) -> String {
        let numbered: String = text
            .lines()
            .enumerate()
            .map(|(i, l)| format!("{:>2}  {l}\n", i + 1))
            .collect();
        e.present(
            &Source::File {
                path: path.into(),
                ranged: false,
            },
            numbered.as_bytes(),
        )
    }

    fn values() -> Vec<&'static str> {
        let mut v: Vec<&str> = NAMES.to_vec();
        v.extend(NAMES.iter().flat_map(|n| n.split(' ')));
        v.extend(EMAILS);
        v.extend(CARDS);
        v.extend(BORN);
        v
    }

    fn assert_no_value(what: &str, text: &str) {
        for v in values() {
            assert!(!text.contains(v), "{v} in {what}:\n{text}");
        }
        for c in CARDS {
            for w in c.as_bytes().windows(8) {
                let run = std::str::from_utf8(w).unwrap();
                assert!(
                    !text.contains(run),
                    "digits {run} of {c} in {what}:\n{text}"
                );
            }
        }
    }

    #[test]
    fn a_data_files_view_has_its_structure_and_a_sample_without_values() {
        let (_d, _ws, e) = primed(views(None));
        let view = read(&e, "data/customers.csv", &csv());
        assert_no_value("the view", &view);
        for want in [
            "Structure (CSV",
            "CSV: 3 rows of 6 fields after a header row; no field quoted",
            "  born — string; 3; all distinct; len 10; date yyyy-MM-dd ×3",
            "  email — string; 3; all distinct;",
            "looks like email ×3",
            "  card — string; 3; all distinct; len 16; shapes 9999999999999999 ×3; looks like card ×3",
            "  status — string; 3; 2-5 distinct; len 7; shapes aaaaaaa ×3",
            "Synthetic sample (record 1 of 3;",
            "id,name,born,email,card,status\n",
        ] {
            assert!(view.contains(want), "missing {want:?}:\n{view}");
        }
        // The sample's card number is fresh and passes the check.
        let row = view
            .lines()
            .skip_while(|l| !l.starts_with("id,name"))
            .nth(1)
            .unwrap();
        let card = row.split(',').nth(4).unwrap();
        assert_eq!(card.len(), 16);
        assert!(
            crate::detect::scan(card, crate::detect::Detectors::default())
                .iter()
                .any(|f| f.kind == Kind::Card)
        );
        // Shown as written through the gate: it is the frontier's own now.
        assert!(e.lock().authored.contains(card), "{card}");
        let events = e.take_events();
        assert!(events.iter().any(|ev| matches!(ev,
            AuditEvent::SyntheticSample { outcome, records: 1, .. } if outcome == "shown")));
    }

    #[test]
    fn synthetic_sample_gives_records_and_refuses_other_content() {
        let (_d, _ws, e) = primed(views(None));
        let _ = read(&e, "data/customers.csv", &csv());
        let call = |args: serde_json::Value| {
            e.call_tool("synthetic_sample", args.as_object().unwrap())
                .unwrap()
        };
        let sample = call(serde_json::json!({"handle": "h1", "rows": 50})).unwrap();
        assert!(sample.contains("records 1, 2, 3 of 3"), "{sample}");
        assert_no_value("the sample", &sample);
        assert_eq!(e.take_view_class(), Some(crate::view::ViewClass::Tokenized));
        assert!(call(serde_json::json!({"handle": "h9"})).is_err());
        // Output of a command that read sensitive data is not a data file.
        let out = e.present(
            &Source::SensitiveCommand {
                command: "cat data/customers.csv".into(),
                exit_code: Some(0),
            },
            format!("exit code 0\n--- stdout ---\n{}", csv().repeat(3)).as_bytes(),
        );
        let h = out.split_whitespace().next().unwrap();
        let refused = call(serde_json::json!({"handle": h})).unwrap_err();
        assert!(refused.contains("not a sensitive file"), "{refused}");
    }

    #[test]
    fn a_sample_that_copies_sensitive_text_is_withheld() {
        // Keys nested thirty deep: every sample repeats a long run of the
        // file's own text, which the copy check refuses.
        let mut record = String::from("1");
        for i in (0..30).rev() {
            record = format!("{{\"k{i}\": {record}}}");
        }
        let json = format!("[{record}, {record}]");
        let d = tempfile::tempdir().unwrap();
        let e = Engine::open(d.path(), views(None), None).unwrap();
        let _ = read(&e, "data/deep.json", &json);
        let r = e
            .call_tool(
                "synthetic_sample",
                serde_json::json!({"handle": "h1"}).as_object().unwrap(),
            )
            .unwrap();
        let err = r.unwrap_err();
        assert!(err.contains("withheld") && err.contains("copied"), "{err}");
        assert!(e.take_events().iter().any(|ev| matches!(ev,
            AuditEvent::SyntheticSample { outcome, .. } if outcome == "withheld")));
    }

    fn command(e: &Engine, cmd: &str, out: &str) -> String {
        e.present(
            &Source::SensitiveCommand {
                command: cmd.into(),
                exit_code: Some(0),
            },
            format!("exit code 0\n--- stdout ---\n{out}").as_bytes(),
        )
    }

    #[test]
    fn short_outputs_of_sensitive_commands_are_budgeted_probes() {
        let (_d, _ws, e) = primed(views(Some(2)));
        let first = command(&e, "grep -q Vakdril data/customers.csv && echo M", "M\n");
        assert!(first.contains("Output with every value masked"), "{first}");
        let _ = command(&e, "grep -c pending data/customers.csv", "1\n");
        // Past the budget the view no longer depends on the output.
        let a = command(&e, "grep -q Orla data/customers.csv && echo M", "M\n");
        let b = command(&e, "grep -q Orla data/customers.csv && echo M", "");
        assert_eq!(a, b);
        assert!(a.contains("withheld") && !a.contains("h3"), "{a}");
        let probes: Vec<(u32, bool)> = e
            .take_events()
            .into_iter()
            .filter_map(|ev| match ev {
                AuditEvent::OutputProbe { count, shown, .. } => Some((count, shown)),
                _ => None,
            })
            .collect();
        assert_eq!(probes, vec![(1, true), (2, true), (3, false), (4, false)]);
        // Long output is not a probe.
        let long = command(&e, "cat data/customers.csv", &csv().repeat(4));
        assert!(long.starts_with('h'), "{long}");
    }

    #[test]
    fn masked_output_shows_structure_and_budgets_small_numbers() {
        let (_d, _ws, e) = primed(views(None));
        let out = format!(
            "rows: 3\nparsed {} <{}> born {} card {} status settled\nfailed at line 12, column 7, offset 1234\n{}",
            NAMES[0],
            EMAILS[0],
            BORN[0],
            CARDS[0],
            "padding line to make this output long enough to not be a probe\n".repeat(4)
        );
        let view = command(&e, "node parse.js data/customers.csv", &out);
        assert_no_value("masked output", &view);
        for want in [
            "  rows: 3\n",
            "  a Aa Aa <a.a@a-a.a> born 9999-99-99 card 9999999999999999 status a\n",
            "  failed at line 12, column 7, offset 9999\n",
        ] {
            assert!(view.contains(want), "missing {want:?}:\n{view}");
        }
        // The budget (3) is spent: the next small number is a shape.
        let again = command(&e, "node parse.js data/customers.csv", &out);
        assert!(again.contains("  rows: 9\n"), "{again}");
        let spent: Vec<u32> = e
            .take_events()
            .into_iter()
            .filter_map(|ev| match ev {
                AuditEvent::MaskedNumbers { shown, .. } => Some(shown),
                _ => None,
            })
            .collect();
        assert_eq!(spent, vec![3]);
    }

    #[test]
    fn a_file_written_from_sample_lines_is_a_fixture_until_it_changes() {
        let (_d, ws, e) = primed(views(None));
        let _ = read(&e, "data/customers.csv", &csv());
        let sample = e
            .call_tool(
                "synthetic_sample",
                serde_json::json!({"handle": "h1", "rows": 2})
                    .as_object()
                    .unwrap(),
            )
            .unwrap()
            .unwrap();
        let fixture: String = sample.lines().skip(1).map(|l| format!("{l}\n")).collect();
        let path = Path::new("tests/fixtures/customers.csv");
        assert!(e.path_sensitive(path), "*.csv is sensitive by policy");
        let written = e.resolve_for_write(path, &fixture).unwrap();
        std::fs::create_dir_all(ws.join("tests/fixtures")).unwrap();
        std::fs::write(ws.join(path), &written).unwrap();
        assert!(!e.path_sensitive(path), "a fixture of sample lines");
        assert!(!e.hidden_from_commands(&ws).contains(&ws.join(path)));
        // Its view is the content itself, fakes shown as written.
        let shown = read(&e, "tests/fixtures/customers.csv", &written);
        assert!(shown.contains("id,name,born,email,card,status"), "{shown}");
        assert!(!shown.contains('⟨'), "{shown}");
        // Changed by anything else, it is sensitive again.
        std::fs::write(ws.join(path), format!("{written}9,x\n")).unwrap();
        assert!(e.path_sensitive(path));
        // A write that is not all sample lines is not a fixture.
        let mixed = format!("{fixture}{}\n", csv().lines().nth(1).unwrap());
        let other = Path::new("tests/fixtures/mixed.csv");
        let w = e.resolve_for_write(other, &mixed).unwrap();
        std::fs::write(ws.join(other), w).unwrap();
        assert!(e.path_sensitive(other));
    }

    #[test]
    fn a_data_file_a_command_rewrote_gets_no_sample_or_outline() {
        let (_d, ws, e) = primed(views(Some(0)));
        std::fs::write(ws.join("data/customers.csv"), "M\n").unwrap();
        e.mark_sensitive(&ws, &[PathBuf::from("data/customers.csv")]);
        assert!(!e.task_notes().contains("data/customers.csv:"));
        // Its short content is a probe (budget 0: withheld).
        let view = read(&e, "data/customers.csv", "M\n");
        assert!(view.contains("withheld"), "{view}");
        let _ = read(&e, "data/customers.csv", &csv().repeat(3));
        let r = e
            .call_tool(
                "synthetic_sample",
                serde_json::json!({"handle": "h1"}).as_object().unwrap(),
            )
            .unwrap();
        assert!(r.unwrap_err().contains("not a sensitive file"));
    }

    #[test]
    fn env_files_get_value_formats_and_the_task_an_outline() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("ws");
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::write(
            ws.join(".env"),
            "# prod\nexport PAYMENTS_API_KEY=\"sk_live_Qm8vT2xW9pL4nR7kZ3cY6bH1\"\nLIMIT=5000\nbroken line here\n",
        )
        .unwrap();
        std::fs::write(ws.join("data/customers.csv"), csv()).unwrap();
        let e = Engine::open(&d.path().join("run"), views(None), None).unwrap();
        e.prime(&ws, &["data/customers.csv".into()], "Load the settings.");
        let env = std::fs::read_to_string(ws.join(".env")).unwrap();
        let view = read(&e, ".env", &env);
        for want in [
            "PAYMENTS_API_KEY — double-quoted; exported; secret-like, 32 chars: A-Z a-z 0-9 _",
            "LIMIT — integer, 4 digits",
            "1 line(s) that are neither assignments nor comments: a a a ×1",
        ] {
            assert!(view.contains(want), "missing {want:?}:\n{view}");
        }
        // The key is withheld; a limit is a plainly public setting, shown as it is.
        assert!(
            !view.contains("Qm8v") && view.contains("LIMIT=5000"),
            "{view}"
        );
        let task = e.sanitize_objective("Load the settings.");
        assert!(task.contains("Structure of the sensitive files"), "{task}");
        assert!(
            task.contains(
                "- .env: KEY=value, 2 assignments: PAYMENTS_API_KEY (double-quoted; exported;"
            ),
            "{task}"
        );
        assert!(
            task.contains("- data/customers.csv: CSV, 3 rows of 6 fields after a header row"),
            "{task}"
        );
        assert_no_value("the task", &task);
    }
}
