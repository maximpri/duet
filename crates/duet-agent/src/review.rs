// SPDX-License-Identifier: GPL-3.0-or-later
//! Run-start snapshots and the finish-time auditor. The baseline is independent
//! of git and of the write journal, so command writes and dirty starts count.
//! All raw state stays in the private run directory and is purged with the run.

use duet_boundary::audit::{AuditEvent, AuditHandle};
use duet_boundary::view::{Explored, Presenter, Source};
use duet_review::{Severity, Verdict};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_FILE: u64 = 256 * 1024;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;
const BASELINE: &str = "review-baseline.json";
const REPORT: &str = "security-review.json";

#[derive(Clone)]
pub struct Settings {
    pub enabled: bool,
    pub block_high: bool,
    pub max_candidates: usize,
    pub local_open: bool,
    pub rules_only: bool,
    pub second: Option<Arc<duet_boundary::review::SecondReviewer>>,
    pub max_second_candidates: usize,
    pub remaining_usd: f64,
    pub lsp: Option<Arc<duet_lsp::Lsp>>,
    pub scanners: Vec<external::Scanner>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            block_high: false,
            max_candidates: 16,
            local_open: false,
            rules_only: false,
            second: None,
            max_second_candidates: 4,
            remaining_usd: f64::INFINITY,
            lsp: None,
            scanners: Vec::new(),
        }
    }
}

mod context;
pub mod external;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct File {
    digest: String,
    source: Option<String>,
}
#[derive(Default, Serialize, Deserialize, PartialEq, Eq)]
struct Snapshot {
    files: BTreeMap<PathBuf, File>,
    /// A cap, I/O failure, invalid UTF-8 or a symlink. Never called a clean scan.
    incomplete: bool,
    excluded_directories: usize,
}

fn extension(path: &Path) -> &str {
    path.extension().and_then(|e| e.to_str()).unwrap_or("")
}

fn snapshot(workspace: &Path) -> Snapshot {
    let mut out = Snapshot::default();
    let mut todo = vec![PathBuf::new()];
    let (mut entries, mut bytes) = (0, 0);
    while let Some(dir) = todo.pop() {
        let Ok(list) = std::fs::read_dir(workspace.join(&dir)) else {
            out.incomplete = true;
            continue;
        };
        // Bound collection itself, including repositories with huge directories.
        let mut list: Vec<_> = list.take(MAX_ENTRIES.saturating_sub(entries) + 1).collect();
        entries += list.len();
        if entries > MAX_ENTRIES {
            out.incomplete = true;
            break;
        }
        list.sort_by_key(|e| e.as_ref().ok().map(|e| e.file_name()));
        for entry in list {
            let Ok(entry) = entry else {
                out.incomplete = true;
                continue;
            };
            let rel = dir.join(entry.file_name());
            if duet_fs::is_reserved(&rel) {
                continue;
            }
            if rel.to_str().is_none() {
                out.incomplete = true;
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                out.incomplete = true;
                continue;
            };
            if kind.is_dir() {
                if [
                    "node_modules",
                    "target",
                    ".venv",
                    "venv",
                    "vendor",
                    "__pycache__",
                ]
                .iter()
                .any(|s| entry.file_name() == *s)
                {
                    out.excluded_directories += 1;
                } else {
                    todo.push(rel);
                }
                continue;
            }
            if !kind.is_file() {
                out.incomplete = true;
                continue;
            }
            let content = match duet_fs::read_file(workspace, &rel, MAX_FILE) {
                Ok(b) => b,
                Err(_) => {
                    out.incomplete = true;
                    continue;
                }
            };
            bytes += content.len();
            if bytes > MAX_BYTES {
                out.incomplete = true;
                return out;
            }
            let digest = duet_fs::sha256_hex(&content);
            let source = String::from_utf8(content).ok();
            out.incomplete |= source.is_none() && duet_review::supported(extension(&rel));
            out.files.insert(rel, File { digest, source });
        }
    }
    out
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Baseline {
    revision: u32,
    workspace: PathBuf,
    snapshot: Snapshot,
}
impl Baseline {
    pub(crate) fn open(
        workspace: &Path,
        run_dir: &Path,
        resumed: bool,
        wait: Option<&dyn duet_fs::host::HostWait>,
    ) -> Result<Self, String> {
        let workspace = workspace
            .canonicalize()
            .map_err(|_| "cannot resolve security review workspace")?;
        match std::fs::symlink_metadata(run_dir.join(BASELINE)) {
            Ok(_) => {
                let bytes =
                    duet_fs::read_file(run_dir, Path::new(BASELINE), (MAX_BYTES * 8) as u64)
                        .map_err(|_| "cannot read security review baseline")?;
                let saved: Self = serde_json::from_slice(&bytes)
                    .map_err(|_| "invalid security review baseline")?;
                if saved.revision != duet_review::REVISION || saved.workspace != workspace {
                    return Err(
                        "security review baseline belongs to another workspace or rules revision"
                            .into(),
                    );
                }
                Ok(saved)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !resumed => {
                let saved = Self {
                    revision: duet_review::REVISION,
                    snapshot: snapshot(&workspace),
                    workspace,
                };
                duet_fs::host::persist(wait, || {
                    duet_fs::private::write_private(
                        &run_dir.join(BASELINE),
                        &serde_json::to_vec(&saved).unwrap(),
                    )
                })
                .map_err(|_| "cannot store security review baseline")?;
                Ok(saved)
            }
            _ => {
                Err("security review baseline is missing; start a new run to enable review".into())
            }
        }
    }
}

#[derive(Serialize)]
struct Finding {
    rule: String,
    severity: Severity,
    confirmed: bool,
    /// Sanitized together with the local opinion, never a raw path or code.
    report: String,
    local_verdict: Option<Verdict>,
    frontier_verdict: Option<Verdict>,
}

#[derive(Serialize)]
struct Report {
    revision: u32,
    changed_files: usize,
    unsupported_changes: usize,
    excluded_directories: usize,
    incomplete: bool,
    local_reviewed: usize,
    local_unavailable: usize,
    frontier_reviewed: usize,
    frontier_unavailable: usize,
    external_candidates: usize,
    external_unavailable: usize,
    referenced_files: usize,
    blocked: bool,
    seconds: f64,
    findings: Vec<Finding>,
    /// Private evidence tying this review to exact file contents. Not in audit.
    reviewed_files: BTreeMap<PathBuf, String>,
}

#[derive(Debug)]
pub struct Reviewed {
    pub blocked: bool,
    pub text: String,
}

/// Local checks have passed before this is called. Cancellation and time are
/// checked between opinions and again before the run can become Completed.
pub(crate) async fn finish(
    base: &Baseline,
    run_dir: &Path,
    settings: Settings,
    presenter: &dyn Presenter,
    audit: &AuditHandle,
    stopped: impl Fn() -> bool + Sync,
    wait: Option<&dyn duet_fs::host::HostWait>,
) -> Result<Reviewed, String> {
    let start = std::time::Instant::now();
    let current = snapshot(&base.workspace);
    let paths: BTreeSet<_> = base
        .snapshot
        .files
        .keys()
        .chain(current.files.keys())
        .collect();
    let fields = presenter.review_fields();
    let mut report = Report {
        revision: duet_review::REVISION,
        changed_files: 0,
        unsupported_changes: 0,
        excluded_directories: current.excluded_directories,
        incomplete: base.snapshot.incomplete || current.incomplete,
        local_reviewed: 0,
        local_unavailable: 0,
        frontier_reviewed: 0,
        frontier_unavailable: 0,
        external_candidates: 0,
        external_unavailable: 0,
        referenced_files: 0,
        blocked: false,
        seconds: 0.0,
        findings: Vec::new(),
        reviewed_files: BTreeMap::new(),
    };
    let mut context_budget = settings.max_candidates.min(duet_review::MAX_CANDIDATES);
    let mut second_budget = settings
        .max_second_candidates
        .min(duet_review::MAX_CANDIDATES);
    let second_start = settings.second.as_ref().map_or(0.0, |s| s.spent_usd());
    let baseline_hosts: BTreeSet<_> = base
        .snapshot
        .files
        .iter()
        .flat_map(|(p, f)| {
            f.source
                .as_deref()
                .map(|s| duet_review::hosts(extension(p), s))
                .unwrap_or_default()
        })
        .collect();
    let external = external::scan_changes(
        &base.workspace,
        &base.snapshot,
        &current,
        &settings.scanners,
        &stopped,
        run_dir,
    )
    .await;
    report.external_unavailable = external.unavailable;
    report.incomplete |= external.unavailable > 0;
    for path in paths {
        if stopped() {
            return Err("security review interrupted or out of time".into());
        }
        let old = base.snapshot.files.get(path);
        let new = current.files.get(path);
        let changed = old.map(|f| &f.digest) != new.map(|f| &f.digest);
        let extra = external.findings.get(path).filter(|v| !v.is_empty());
        // A change in another file can introduce a finding at this unchanged
        // location. The scanner has already subtracted its baseline findings.
        if !changed && extra.is_none() {
            continue;
        }
        report.changed_files += usize::from(changed);
        if let Some(new) = new {
            report
                .reviewed_files
                .insert(path.clone(), new.digest.clone());
        }
        if !duet_review::supported(extension(path)) && extra.is_none() {
            report.unsupported_changes += 1;
            continue;
        }
        let source = new.and_then(|f| f.source.as_deref()).unwrap_or("");
        let old_source = old.and_then(|f| f.source.as_deref()).unwrap_or("");
        let after = duet_review::scan_with_values(
            extension(path),
            source,
            &fields,
            &presenter.review_value_spans(source),
        );
        let before = old
            .and_then(|f| f.source.as_ref())
            .map(|s| {
                duet_review::scan_with_values(
                    extension(path),
                    s,
                    &fields,
                    &presenter.review_value_spans(s),
                )
            })
            .unwrap_or_default();
        let mut scan = duet_review::introduced(before, after);
        scan.candidates.retain(|c| {
            c.rule != "outbound-host"
                || !baseline_hosts.contains(c.identity.rsplit('\0').next().unwrap_or(""))
        });
        scan.candidates.extend(duet_review::removed_guards(
            extension(path),
            old_source,
            source,
        ));
        if let Some(extra) = extra {
            report.external_candidates += extra.len();
            scan.candidates.extend(extra.iter().cloned());
        }
        report.incomplete |= scan.incomplete;
        for mut candidate in scan.candidates {
            let confirmed_high = candidate.confirmed && candidate.severity == Severity::High;
            // Enforcement covers every detected candidate, independently of
            // the report and opinion limits. Keep blockers actionable by
            // replacing an advisory entry if the report is already full.
            report.blocked |= settings.block_high && confirmed_high;
            if report.findings.len() >= duet_review::MAX_CANDIDATES {
                report.incomplete = true;
                let replace = confirmed_high
                    .then(|| {
                        report
                            .findings
                            .iter()
                            .rposition(|f| !(f.confirmed && f.severity == Severity::High))
                    })
                    .flatten();
                if let Some(index) = replace {
                    report.findings.remove(index);
                } else {
                    continue;
                }
            }
            let mut local_verdict = None;
            let mut frontier_verdict = None;
            let mut read = vec![
                (Some(path.clone()), source.to_owned()),
                (Some(path.clone()), old_source.to_owned()),
            ];
            if let Some(lsp) = &settings.lsp {
                let references = context::gather(
                    &base.workspace,
                    &current,
                    path,
                    &candidate,
                    lsp,
                    presenter,
                    &stopped,
                )
                .await;
                for (p, snippet) in references {
                    if candidate.context.len() + snippet.len() + 30 > duet_review::MAX_CONTEXT {
                        report.incomplete = true;
                        break;
                    }
                    candidate.context.push_str("\n\nReferenced context:\n");
                    candidate.context.push_str(&snippet);
                    read.push((Some(p), snippet));
                    report.referenced_files += 1;
                }
                for e in lsp.take_events() {
                    audit.record(AuditEvent::LanguageServer {
                        language: e.language,
                        op: "server".into(),
                        path: None,
                        outcome: e.what,
                        shown: 0,
                        withheld: 0,
                    });
                }
            }
            let mut text = format!(
                "{} at {}:{}: {} [rule-confirmed: {}]",
                candidate.rule,
                if presenter.path_visible(path) {
                    path.display().to_string()
                } else {
                    "protected file".into()
                },
                candidate.line,
                candidate.message,
                candidate.confirmed
            );
            let private_sources = read.iter().any(|(p, s)| {
                !presenter.review_value_spans(s).is_empty()
                    || fields
                        .iter()
                        .any(|field| !field.is_empty() && s.contains(field))
                    || p.as_ref().is_some_and(|p| {
                        presenter.path_sensitive(p)
                            || presenter.protection(p).is_some()
                            || duet_review::scan(extension(p), s, &fields)
                                .candidates
                                .iter()
                                .any(|c| matches!(c.rule, "sensitive-sink" | "credential-literal"))
                    })
            });
            let local_eligible = !settings.rules_only
                && (settings.local_open
                    || private_sources
                    || presenter.path_sensitive(path)
                    || presenter.protection(path).is_some()
                    || candidate.known_private_value
                    || matches!(candidate.rule, "sensitive-sink" | "credential-literal"));
            if local_eligible && context_budget > 0 {
                context_budget -= 1;
                match presenter.review_candidate(&candidate) {
                    Some(Ok(j)) => {
                        report.local_reviewed += 1;
                        text.push_str(&format!(
                            "\nLocal opinion ({:?}): {}\nSuggested fix: {}",
                            j.verdict, j.explanation, j.fix
                        ));
                        local_verdict = Some(j.verdict);
                    }
                    _ => {
                        report.local_unavailable += 1;
                        text.push_str("\nLocal opinion unavailable; rule finding retained.");
                    }
                }
            } else if local_eligible {
                report.local_unavailable += 1;
                text.push_str("\nLocal review candidate budget exhausted; rule finding retained.");
            }
            if !settings.rules_only
                && !private_sources
                && settings.second.is_some()
                && !["sensitive-sink", "credential-literal", "external-scanner"]
                    .contains(&candidate.rule)
                && (candidate.severity == Severity::High
                    || local_verdict == Some(Verdict::Uncertain))
            {
                let paths: Vec<_> = read.iter().filter_map(|(p, _)| p.clone()).collect();
                // Whole source versions were checked above before selecting a
                // bounded location window. The boundary checks the final view again.
                let before_context =
                    duet_review::context::at(extension(path), old_source, candidate.line);
                let cap = duet_review::MAX_CONTEXT / 2 - 256;
                let diff = serde_json::json!({"before":duet_review::context::prefix(&before_context, cap),"after":duet_review::context::prefix(&candidate.context, cap)}).to_string();
                let remaining = settings.remaining_usd
                    - (settings.second.as_ref().map_or(0.0, |s| s.spent_usd()) - second_start);
                let opinion = if second_budget == 0 || diff.len() > duet_review::MAX_CONTEXT {
                    Some(Err("security second opinion limit reached".into()))
                } else {
                    let result = presenter.review_second(&candidate, &paths, &diff, remaining);
                    if result.is_some() {
                        second_budget -= 1;
                    }
                    result
                };
                if let Some(opinion) = opinion {
                    match opinion {
                        Ok(j) => {
                            report.frontier_reviewed += 1;
                            text.push_str(&format!(
                                "\nFrontier opinion ({:?}): {}\nSuggested fix: {}",
                                j.verdict, j.explanation, j.fix
                            ));
                            frontier_verdict = Some(j.verdict);
                        }
                        Err(_) => {
                            report.frontier_unavailable += 1;
                            text.push_str("\nFrontier opinion unavailable; rule finding retained.");
                        }
                    }
                }
            }
            // The same copied-span, re-encoding, sensitive-value and protected
            // code filters used for local exploration. The next frontier
            // request still passes the outbound gate as every request does.
            let text = presenter.present(&Source::Explore {
                question: "Review this security candidate and suggest a safe fix without quoting private content".into(),
                read: Explored(Arc::new(read)),
            }, text.as_bytes());
            for event in presenter.take_events() {
                audit.record(event);
            }
            report.findings.push(Finding {
                rule: candidate.rule.into(),
                severity: candidate.severity,
                confirmed: candidate.confirmed,
                report: text,
                local_verdict,
                frontier_verdict,
            });
        }
    }
    // Concurrent writes during a local opinion invalidate this completion.
    // Also catches a new file, deletion or excluded-directory coverage change.
    if snapshot(&base.workspace) != current {
        return Err(
            "workspace changed during security review; finish again to review the new state".into(),
        );
    }
    if stopped() {
        return Err("security review interrupted or out of time".into());
    }
    report.incomplete |= report.unsupported_changes > 0
        || report.local_unavailable > 0
        || report.frontier_unavailable > 0;
    report.seconds = start.elapsed().as_secs_f64();
    duet_fs::host::persist(wait, || {
        duet_fs::private::write_private(
            &run_dir.join(REPORT),
            &serde_json::to_vec_pretty(&report).unwrap(),
        )
    })
    .map_err(|_| "cannot store security review report")?;
    let rules: BTreeSet<_> = report.findings.iter().map(|f| f.rule.clone()).collect();
    audit.record(AuditEvent::SecurityReview {
        changed_files: report.changed_files,
        candidates: report.findings.len(),
        high: report
            .findings
            .iter()
            .filter(|f| f.severity == Severity::High)
            .count(),
        local_reviewed: report.local_reviewed,
        medium: report
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Medium)
            .count(),
        local_unavailable: report.local_unavailable,
        frontier_reviewed: report.frontier_reviewed,
        frontier_unavailable: report.frontier_unavailable,
        external_candidates: report.external_candidates,
        external_unavailable: report.external_unavailable,
        referenced_files: report.referenced_files,
        incomplete: report.incomplete,
        blocked: report.blocked,
        rule_ids: rules.into_iter().collect(),
    });
    let mut text = format!(
        "Security review (bounded rules v{}): {} changed files, {} new candidates, {} local opinions. Coverage: {}; {} dependency/build directories excluded. This is not a full security assessment.",
        duet_review::REVISION,
        report.changed_files,
        report.findings.len(),
        report.local_reviewed,
        if report.incomplete {
            "incomplete"
        } else {
            "initial rule set only"
        },
        report.excluded_directories
    );
    for finding in &report.findings {
        if text.len() + finding.report.len() > 32_000 {
            text.push_str("\nFurther findings are in the private security-review.json report.");
            break;
        }
        text.push_str("\n\n");
        text.push_str(&finding.report);
    }
    if !settings.scanners.is_empty() {
        text.push_str("\nFull external scanner output is kept privately under security-scanners/ in this run directory; it is not sent to a model.");
    }
    Ok(Reviewed {
        blocked: report.blocked,
        text,
    })
}

/// The repository scanner and finish-time auditor use the same pipeline. An
/// empty baseline means every current candidate is in scope.
pub async fn repository(
    workspace: &Path,
    run_dir: &Path,
    settings: Settings,
    presenter: &dyn Presenter,
    audit: &AuditHandle,
    stopped: impl Fn() -> bool + Sync,
) -> Result<Reviewed, String> {
    let base = Baseline {
        revision: duet_review::REVISION,
        workspace: workspace
            .canonicalize()
            .map_err(|_| "cannot resolve scan workspace")?,
        snapshot: Snapshot::default(),
    };
    finish(&base, run_dir, settings, presenter, audit, stopped, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use duet_boundary::audit::AuditLog;
    use duet_boundary::view::PassThrough;
    use std::os::unix::fs::PermissionsExt;

    fn finish(
        base: &Baseline,
        run: &Path,
        settings: Settings,
        presenter: &dyn Presenter,
        audit: &AuditHandle,
        stopped: impl Fn() -> bool + Sync,
        wait: Option<&dyn duet_fs::host::HostWait>,
    ) -> Result<Reviewed, String> {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(super::finish(
                base, run, settings, presenter, audit, stopped, wait,
            ))
    }

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, AuditHandle) {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("ws");
        let run = ws.join(".duet/runs/test");
        std::fs::create_dir_all(&run).unwrap();
        let audit = AuditHandle::new(AuditLog::open(&d.path().join("audit.jsonl")).unwrap());
        (d, ws, run, audit)
    }
    fn settings() -> Settings {
        Settings {
            enabled: true,
            block_high: true,
            max_candidates: 16,
            local_open: true,
            ..Settings::default()
        }
    }
    const BAD: &str = "import requests\nrequests.get(url, verify=False)\n";

    #[test]
    fn host_inventory_and_deleted_authorization_checks_are_compared_to_the_baseline() {
        let (_d, ws, run, audit) = setup();
        std::fs::write(
            ws.join("existing.py"),
            "requests.get('https://service.example.org/a')\n",
        )
        .unwrap();
        std::fs::write(
            ws.join("auth.py"),
            "@login_required\ndef handler():\n    return 'ok'\n",
        )
        .unwrap();
        let base = Baseline::open(&ws, &run, false, None).unwrap();
        std::fs::write(
            ws.join("new.py"),
            "requests.post('https://service.example.org/b')\n",
        )
        .unwrap();
        std::fs::remove_file(ws.join("auth.py")).unwrap();
        let r = finish(
            &base,
            &run,
            settings(),
            &PassThrough { max_bytes: 60000 },
            &audit,
            || false,
            None,
        )
        .unwrap();
        assert!(!r.blocked);
        assert!(r.text.contains("auth-guard-removed"));
        assert!(!r.text.contains("outbound-host"));
    }

    #[test]
    fn finish_uses_private_value_hints_and_filters_the_report() {
        use duet_boundary::engine::Engine;
        use duet_boundary::policy::{Policy, StructureSettings};
        let (_d, ws, run, audit) = setup();
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::write(
            ws.join("data/customer.csv"),
            "birth_date,card\n1994-11-07,5293761582049377\n",
        )
        .unwrap();
        let engine = Engine::open(
            &run,
            Policy {
                sensitive_globs: vec!["data/**".into()],
                sealed: vec!["private.py".into()],
                detect_pii: true,
                detect_secrets: true,
                structure: StructureSettings {
                    views: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        )
        .unwrap();
        engine.prime(&ws, &["data/customer.csv".into()], "review changes");
        let base = Baseline::open(&ws, &run, false, None).unwrap();
        std::fs::write(
            ws.join("private.py"),
            "def hidden_transform_7ca91f():\n    value = '1994-11-07'\n    logger.info(value)\n",
        )
        .unwrap();
        let reviewed = finish(
            &base,
            &run,
            settings(),
            engine.as_ref(),
            &audit,
            || false,
            None,
        )
        .unwrap();
        assert!(!reviewed.blocked);
        assert!(
            reviewed.text.contains("1 new candidates"),
            "{}",
            reviewed.text
        );
        assert!(reviewed.text.contains("sensitive-sink"));
        assert!(reviewed.text.contains("Coverage: incomplete")); // no local reader
        let persisted = std::fs::read_to_string(run.join(REPORT)).unwrap();
        for secret in [
            "1994-11-07",
            "hidden_transform_7ca91f",
            "logger.info(value)",
        ] {
            assert!(!reviewed.text.contains(secret));
            assert!(!persisted.contains(secret));
        }
    }

    #[test]
    fn dirty_start_command_writes_resume_and_fix() {
        let (_d, ws, run, audit) = setup();
        std::fs::write(ws.join("old.py"), BAD).unwrap();
        let base = Baseline::open(&ws, &run, false, None).unwrap();
        assert_eq!(
            std::fs::metadata(run.join(BASELINE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        // A command write (no journal entry), also absent from any git listing.
        std::fs::write(ws.join(".gitignore"), "new.py\n").unwrap();
        std::fs::write(ws.join("new.py"), BAD).unwrap();
        let base = Baseline::open(&ws, &run, true, None).unwrap_or(base);
        let r = finish(
            &base,
            &run,
            settings(),
            &PassThrough { max_bytes: 20000 },
            &audit,
            || false,
            None,
        )
        .unwrap();
        assert!(r.blocked);
        assert!(r.text.contains("new.py"));
        assert!(!r.text.contains("old.py"));
        std::fs::write(ws.join("new.py"), BAD.replace("False", "True")).unwrap();
        let r = finish(
            &base,
            &run,
            settings(),
            &PassThrough { max_bytes: 20000 },
            &audit,
            || false,
            None,
        )
        .unwrap();
        assert!(!r.blocked);
        assert!(r.text.contains("0 new candidates"));
        std::fs::write(run.join(BASELINE), b"broken").unwrap();
        assert!(Baseline::open(&ws, &run, true, None).is_err());
        std::fs::remove_file(run.join(BASELINE)).unwrap();
        assert!(Baseline::open(&ws, &run, true, None).is_err());
    }

    #[test]
    fn unsupported_symlink_large_file_and_cancellation_are_not_clean() {
        let (d, ws, run, audit) = setup();
        let base = Baseline::open(&ws, &run, false, None).unwrap();
        // macOS filesystems reject these names at creation; Linux permits them.
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::ffi::OsStringExt;
            let invalid_name = std::ffi::OsString::from_vec(vec![0xff, b'.', b'p', b'y']);
            std::fs::write(ws.join(invalid_name), BAD).unwrap();
        }
        std::fs::write(ws.join("A.java"), "class A {}").unwrap();
        std::fs::write(d.path().join("outside.py"), BAD).unwrap();
        std::os::unix::fs::symlink(d.path().join("outside.py"), ws.join("link.py")).unwrap();
        std::fs::write(ws.join("huge.py"), vec![b'x'; MAX_FILE as usize + 1]).unwrap();
        let r = finish(
            &base,
            &run,
            settings(),
            &PassThrough { max_bytes: 20000 },
            &audit,
            || false,
            None,
        )
        .unwrap();
        assert!(!r.blocked);
        assert!(r.text.contains("Coverage: incomplete"));
        assert!(!r.text.contains("outside"));
        assert!(
            finish(
                &base,
                &run,
                settings(),
                &PassThrough { max_bytes: 20000 },
                &audit,
                || true,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn opinions_cannot_veto_or_create_blockers_and_racing_writes_invalidate() {
        struct Opinion {
            ws: Option<PathBuf>,
            verdict: Verdict,
        }
        impl Presenter for Opinion {
            fn present(&self, _: &Source, b: &[u8]) -> String {
                String::from_utf8_lossy(b).into()
            }
            fn review_candidate(
                &self,
                _: &duet_review::Candidate,
            ) -> Option<Result<duet_review::Judgment, String>> {
                if let Some(ws) = &self.ws {
                    std::fs::write(ws.join("raced.py"), BAD).unwrap();
                }
                Some(Ok(duet_review::Judgment {
                    verdict: self.verdict.clone(),
                    severity: duet_review::OpinionSeverity::High,
                    reason: duet_review::Reason::BlockedPath,
                    explanation: "opinion".into(),
                    fix: "advice".into(),
                }))
            }
        }
        let (_d, ws, run, audit) = setup();
        let base = Baseline::open(&ws, &run, false, None).unwrap();
        std::fs::write(ws.join("new.py"), BAD).unwrap();
        let p = Opinion {
            ws: None,
            verdict: Verdict::Unlikely,
        };
        assert!(
            finish(&base, &run, settings(), &p, &audit, || false, None)
                .unwrap()
                .blocked
        );
        let warn = Settings {
            block_high: false,
            ..settings()
        };
        assert!(
            !finish(&base, &run, warn, &p, &audit, || false, None)
                .unwrap()
                .blocked
        );
        std::fs::write(ws.join("new.py"), "os.system(input())").unwrap();
        let p = Opinion {
            ws: None,
            verdict: Verdict::Likely,
        };
        assert!(
            !finish(&base, &run, settings(), &p, &audit, || false, None)
                .unwrap()
                .blocked
        );
        let p = Opinion {
            ws: Some(ws),
            verdict: Verdict::Unlikely,
        };
        assert!(
            finish(&base, &run, settings(), &p, &audit, || false, None)
                .unwrap_err()
                .contains("workspace changed")
        );
    }
}
