// SPDX-License-Identifier: GPL-3.0-or-later
//! Private plan state. Only operator-facing code may approve, implement or resume.
//! Model tool adapters may propose revisions and report progress, never approval.
//! All text is local/private: callers must use the normal message privacy boundary.

use declass_fs::pinned::PinnedParent;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path};

pub type Result<T> = std::result::Result<T, String>;
pub const DEFAULT_MAX_TURNS: u64 = 20;
const MAX_FILE: usize = 4 * 1024 * 1024;
const MAX_TEXT: usize = 64 * 1024;
const MAX_REVISIONS: usize = 100;
const MAX_STEPS: usize = 100;
const MAX_EVIDENCE: usize = 2000;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub title: String,
    pub objective: String,
    pub scope: Vec<String>,
    pub non_goals: Vec<String>,
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub decisions: Vec<String>,
    pub steps: Vec<Step>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub id: String,
    pub title: String,
    pub description: String,
    pub paths: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<String>,
    pub checks: Vec<Check>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub id: String,
    pub command: String,
    pub description: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    pub id: String,
    pub digest: String,
    pub draft: Draft,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub revision: String,
    pub digest: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Active,
    Waiting,
    Paused,
    Completed,
    Exhausted,
    Cancelled,
    Superseded,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    DoneUnverified,
    Verified,
    Blocked,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepProgress {
    pub id: String,
    pub status: StepStatus,
    pub note: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub turns: u64,
    pub max_turns: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationKind {
    Proposed,
    Configured,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Running,
    Passed,
    Failed,
    Invalidated,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verification {
    pub id: String,
    pub revision: String,
    pub step_id: String,
    pub check_id: String,
    pub kind: VerificationKind,
    pub command: String,
    pub status: VerificationStatus,
    pub valid: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub interrupted: bool,
    pub output_digest: Option<String>,
    /// Host wall-clock time when a completed result was recorded, not a duration.
    #[serde(default)]
    pub timestamp_ms: Option<u64>,
    /// Private run-relative capture; never a workspace or absolute path.
    #[serde(default)]
    pub output_path: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationTicket {
    pub id: String,
    pub revision: String,
    pub step_id: String,
    pub check_id: String,
    pub command: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Execution {
    pub revision: String,
    pub digest: String,
    pub status: ExecutionStatus,
    pub turns: u64,
    pub max_turns: u64,
    pub turn_open: bool,
    pub steps: Vec<StepProgress>,
    pub evidence: Vec<Verification>,
    pub reason: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub id: String,
    pub revision: String,
    pub question: String,
    pub answer: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanState {
    pub version: u32,
    pub revisions: Vec<Revision>,
    pub approval: Option<Approval>,
    pub execution: Option<Execution>,
    pub execution_history: Vec<Execution>,
    pub questions: Vec<Question>,
    pub carried_budget: Option<Budget>,
}
impl Default for PlanState {
    fn default() -> Self {
        Self {
            version: 1,
            revisions: Vec::new(),
            approval: None,
            execution: None,
            execution_history: Vec::new(),
            questions: Vec::new(),
            carried_budget: None,
        }
    }
}
impl PlanState {
    pub fn current(&self) -> Option<&Revision> {
        self.revisions.last()
    }
    pub fn active(&self) -> bool {
        self.execution
            .as_ref()
            .is_some_and(|e| e.status == ExecutionStatus::Active)
    }
    pub fn ready_to_finish(&self) -> bool {
        self.execution
            .as_ref()
            .is_some_and(|e| e.status == ExecutionStatus::Active && ready(self, e))
    }
}

pub struct Store {
    file: PinnedParent,
    markdown: PinnedParent,
    lock: PinnedParent,
    saved: PlanState,
    projection_error: Option<String>,
}
impl Store {
    /// No state is created or repaired by loading. Call recover explicitly on
    /// process/session startup, never on each tool-dispatch load.
    pub fn load(run_dir: &Path) -> Result<Self> {
        let root = if run_dir.is_absolute() {
            run_dir.to_owned()
        } else {
            std::env::current_dir().map_err(err)?.join(run_dir)
        };
        ensure(
            root.components()
                .all(|c| matches!(c, Component::RootDir | Component::Normal(_))),
            "plan directory contains traversal",
        )?;
        let rel = root.strip_prefix("/").map_err(err)?;
        let file =
            PinnedParent::open(Path::new("/"), &rel.join("plan.json"), false).map_err(err)?;
        let markdown =
            PinnedParent::open(Path::new("/"), &rel.join("plan.md"), false).map_err(err)?;
        let lock =
            PinnedParent::open(Path::new("/"), &rel.join("plan.lock"), false).map_err(err)?;
        let saved = read(&file)?;
        Ok(Self {
            file,
            markdown,
            lock,
            saved,
            projection_error: None,
        })
    }
    pub fn snapshot(&self) -> &PlanState {
        &self.saved
    }
    pub fn current(&self) -> Option<&Revision> {
        self.saved.current()
    }
    pub fn active(&self) -> bool {
        self.saved.active()
    }
    pub fn ready_to_finish(&self) -> bool {
        self.saved.ready_to_finish()
    }
    /// Canonical save may succeed even if the derived Markdown cannot be
    /// replaced. This diagnostic never changes approval or execution state.
    pub fn projection_error(&self) -> Option<&str> {
        self.projection_error.as_deref()
    }
    pub fn render_markdown(&self) -> String {
        markdown(&self.saved)
    }
    /// Raw private text; enter the session through its ordinary sanitizer.
    pub fn prompt(&self) -> Option<String> {
        let e = self
            .saved
            .execution
            .as_ref()
            .filter(|e| e.status == ExecutionStatus::Active)?;
        Some(format!(
            "Continue the approved plan {} (digest {}). {} of {} turns used. Complete every step; report unverified work honestly and run the saved checks. Ask the operator when blocked.\n\n{}",
            e.revision,
            e.digest,
            e.turns,
            e.max_turns,
            self.render_markdown()
        ))
    }
    pub fn render_markdown_for(&self, revision: &str) -> Result<String> {
        let index = self
            .saved
            .revisions
            .iter()
            .position(|r| r.id == revision)
            .ok_or("unknown plan revision")?;
        let mut state = self.saved.clone();
        state.revisions.truncate(index + 1);
        state.execution = self
            .saved
            .execution
            .iter()
            .chain(&self.saved.execution_history)
            .find(|e| e.revision == revision)
            .cloned();
        Ok(markdown(&state))
    }
    pub fn refresh(&mut self) -> Result<()> {
        self.saved = read(&self.file)?;
        Ok(())
    }
    fn update<T>(&mut self, change: impl FnOnce(&mut PlanState) -> Result<T>) -> Result<T> {
        let lease = acquire(&self.lock)?;
        let mut next = read(&self.file)?;
        let result = change(&mut next)?;
        validate(&next)?;
        let bytes = serde_json::to_vec(&next).map_err(err)?;
        ensure(
            bytes.len() <= MAX_FILE,
            "plan history exceeds 4 MiB; use another session",
        )?;
        regular_or_missing(&self.file)?;
        declass_fs::private::write_private_pinned(&self.file, &bytes).map_err(err)?;
        self.saved = next;
        self.projection_error = regular_or_missing(&self.markdown)
            .and_then(|()| {
                declass_fs::private::write_private_pinned(
                    &self.markdown,
                    self.render_markdown().as_bytes(),
                )
                .map_err(err)
            })
            .err();
        drop(lease);
        Ok(result)
    }
    /// Expected revision makes editor saves compare-and-swap, not last-writer-wins.
    pub fn revise(&mut self, expected: Option<&str>, mut draft: Draft) -> Result<Revision> {
        self.update(|s| {
            assign_ids(s, &mut draft)?;
            validate_draft(&draft)?;
            ensure(
                s.current().map(|r| r.id.as_str()) == expected,
                "the plan changed; review its current revision",
            )?;
            ensure(
                !s.active() && s.execution.as_ref().is_none_or(|e| !e.turn_open),
                "pause execution before revising the plan",
            )?;
            ensure(
                s.revisions.len() < MAX_REVISIONS,
                "plan revision limit reached",
            )?;
            if let Some(mut e) = s.execution.take() {
                if !matches!(
                    e.status,
                    ExecutionStatus::Completed | ExecutionStatus::Cancelled
                ) {
                    s.carried_budget = Some(Budget {
                        turns: e.turns,
                        max_turns: e.max_turns,
                    });
                }
                if !terminal(e.status) {
                    e.status = ExecutionStatus::Superseded;
                }
                s.execution_history.push(e);
            }
            let revision = Revision {
                id: format!("r{}", s.revisions.len() + 1),
                digest: digest(&draft)?,
                draft,
            };
            s.revisions.push(revision.clone());
            s.approval = None;
            Ok(revision)
        })
    }
    /// Operator only. Approves exactly the displayed content, without execution.
    pub fn approve(&mut self, revision: &str, hash: &str) -> Result<()> {
        self.update(|s| {
            current(s, revision, hash)?;
            unanswered(s, revision)?;
            s.approval = Some(Approval {
                revision: revision.into(),
                digest: hash.into(),
            });
            Ok(())
        })
    }
    /// Operator only. Combines exact approval with starting a fresh execution.
    /// Revising an unfinished execution carries its spent turns and ceiling.
    pub fn implement(&mut self, revision: &str, hash: &str, max_turns: u64) -> Result<()> {
        self.update(|s| {
            let r = current(s, revision, hash)?.clone();
            unanswered(s, revision)?;
            ensure(
                s.execution.is_none(),
                "this revision already has an execution; resume it instead",
            )?;
            ensure(
                (1..=1000).contains(&max_turns),
                "plan turn limit must be 1..1000",
            )?;
            let budget = s.carried_budget.clone().unwrap_or(Budget {
                turns: 0,
                max_turns,
            });
            ensure(
                budget.turns < budget.max_turns,
                "plan turn limit is exhausted",
            )?;
            s.approval = Some(Approval {
                revision: revision.into(),
                digest: hash.into(),
            });
            s.execution = Some(Execution {
                revision: revision.into(),
                digest: hash.into(),
                status: ExecutionStatus::Active,
                turns: budget.turns,
                max_turns: budget.max_turns,
                turn_open: false,
                steps: r
                    .draft
                    .steps
                    .iter()
                    .map(|step| StepProgress {
                        id: step.id.clone(),
                        status: StepStatus::Pending,
                        note: None,
                    })
                    .collect(),
                evidence: Vec::new(),
                reason: None,
            });
            s.carried_budget = None;
            Ok(())
        })
    }
    pub fn pause(&mut self, reason: &str) -> Result<()> {
        text(reason, false)?;
        self.update(|s| {
            if let Some(e) = &mut s.execution
                && !terminal(e.status)
            {
                e.status = ExecutionStatus::Paused;
                e.reason = Some(reason.into());
            }
            Ok(())
        })
    }
    pub fn cancel(&mut self) -> Result<()> {
        self.update(|s| {
            let e = execution(s)?;
            ensure(!e.turn_open, "stop the running turn before cancelling")?;
            ensure(!terminal(e.status), "execution has ended")?;
            e.status = ExecutionStatus::Cancelled;
            Ok(())
        })
    }
    pub fn resume(&mut self, revision: &str, hash: &str) -> Result<()> {
        self.update(|s| {
            current(s, revision, hash)?;
            approved(s, revision, hash)?;
            unanswered(s, revision)?;
            let e = execution(s)?;
            ensure(
                matches!(e.status, ExecutionStatus::Paused | ExecutionStatus::Waiting),
                "execution is not paused",
            )?;
            ensure(
                !e.turn_open && e.turns < e.max_turns,
                "execution requires recovery or has exhausted its turn limit",
            )?;
            e.status = ExecutionStatus::Active;
            e.reason = None;
            Ok(())
        })
    }
    /// Explicit process-start recovery: never retries an unknown check or turn.
    pub fn recover(&mut self) -> Result<bool> {
        self.refresh()?;
        let needs_recovery = self.saved.execution.as_ref().is_some_and(|e| {
            e.status == ExecutionStatus::Active
                || e.turn_open
                || e.evidence
                    .iter()
                    .any(|v| v.status == VerificationStatus::Running)
        });
        if !needs_recovery {
            return Ok(false);
        }
        self.update(|s| {
            let mut changed = false;
            if let Some(e) = &mut s.execution {
                if e.status == ExecutionStatus::Active || e.turn_open {
                    e.status = ExecutionStatus::Paused;
                    e.turn_open = false;
                    e.reason = Some("Execution was interrupted; review it before resuming.".into());
                    changed = true;
                }
                for v in &mut e.evidence {
                    if v.status == VerificationStatus::Running {
                        v.status = VerificationStatus::Unknown;
                        v.interrupted = true;
                        v.valid = false;
                        changed = true;
                    }
                }
            }
            Ok(changed)
        })
    }
    pub fn begin_turn(&mut self) -> Result<()> {
        self.update(|s| {
            let r = s.current().ok_or("no saved plan")?;
            approved(s, &r.id, &r.digest)?;
            unanswered(s, &r.id)?;
            let e = execution(s)?;
            ensure(
                e.status == ExecutionStatus::Active && !e.turn_open,
                "no idle active plan execution",
            )?;
            ensure(e.turns < e.max_turns, "plan turn limit is exhausted")?;
            e.turns += 1;
            e.turn_open = true;
            Ok(())
        })
    }
    pub fn finish_turn(&mut self, end: &crate::session::TurnEnd) -> Result<()> {
        self.update(|s| {
            let can_finish = s.ready_to_finish();
            let e = execution(s)?;
            ensure(e.turn_open, "no plan turn is in progress")?;
            e.turn_open = false;
            for v in &mut e.evidence {
                if v.status == VerificationStatus::Running {
                    v.status = VerificationStatus::Unknown;
                    v.interrupted = true;
                    v.valid = false;
                }
            }
            use crate::session::TurnEnd;
            match end {
                TurnEnd::Completed { .. } if can_finish => {
                    e.status = ExecutionStatus::Completed;
                    e.reason = None;
                }
                TurnEnd::Completed { .. } => {
                    e.status = ExecutionStatus::Paused;
                    e.reason = Some("Plan steps or verification remain unfinished.".into());
                }
                TurnEnd::Replied { .. } => {
                    if e.status == ExecutionStatus::Active && e.turns == e.max_turns {
                        e.status = ExecutionStatus::Exhausted;
                        e.reason = Some("Plan turn limit reached.".into());
                    }
                }
                TurnEnd::Asked { .. } => {
                    e.status = ExecutionStatus::Waiting;
                }
                TurnEnd::Failed { reason } => {
                    e.status = ExecutionStatus::Paused;
                    e.reason = Some(reason.clone());
                }
                TurnEnd::BudgetStopped { which } => {
                    e.status = ExecutionStatus::Paused;
                    e.reason = Some(format!("Session limit: {which}"));
                }
                TurnEnd::Interrupted | TurnEnd::Stopped => {
                    e.status = ExecutionStatus::Paused;
                    e.reason = Some("Execution stopped; resume when ready.".into());
                }
            }
            Ok(())
        })
    }
    /// Model progress is a claim. Verified is exclusively derived from host results.
    pub fn report_step(
        &mut self,
        revision: &str,
        step_id: &str,
        status: StepStatus,
        note: Option<&str>,
    ) -> Result<()> {
        ensure(
            status != StepStatus::Verified,
            "only host check results can verify a step",
        )?;
        if let Some(n) = note {
            text(n, false)?;
        }
        self.update(|s| {
            let draft = revision_step(s, revision, step_id)?.clone();
            let e = active_execution(s, revision)?;
            ensure(
                e.turn_open,
                "step progress requires an active reserved turn",
            )?;
            let verified = status == StepStatus::DoneUnverified && checks_pass(e, &draft);
            let p = e
                .steps
                .iter_mut()
                .find(|p| p.id == step_id)
                .ok_or("unknown step")?;
            p.status = if verified {
                StepStatus::Verified
            } else {
                status
            };
            p.note = note.map(str::to_owned);
            Ok(())
        })
    }
    /// Returns only the command already saved in the exact approved revision.
    pub fn begin_verification(
        &mut self,
        revision: &str,
        step_id: &str,
        check_id: &str,
    ) -> Result<VerificationTicket> {
        self.update(|s| {
            let step = revision_step(s, revision, step_id)?;
            let check = step
                .checks
                .iter()
                .find(|c| c.id == check_id)
                .ok_or("unknown plan check")?
                .clone();
            let e = active_execution(s, revision)?;
            ensure(e.turn_open, "a check requires an active reserved turn")?;
            ensure(
                !e.evidence
                    .iter()
                    .any(|v| v.status == VerificationStatus::Running),
                "a check is already running",
            )?;
            let id = format!("v{}", e.evidence.len() + 1);
            let ticket = VerificationTicket {
                id: id.clone(),
                revision: revision.into(),
                step_id: step_id.into(),
                check_id: check_id.into(),
                command: check.command.clone(),
            };
            e.evidence.push(Verification {
                id,
                revision: revision.into(),
                step_id: step_id.into(),
                check_id: check_id.into(),
                kind: VerificationKind::Proposed,
                command: check.command,
                status: VerificationStatus::Running,
                valid: true,
                exit_code: None,
                timed_out: false,
                interrupted: false,
                output_digest: None,
                timestamp_ms: None,
                output_path: None,
            });
            if let Some(p) = e.steps.iter_mut().find(|p| p.id == step_id)
                && p.status == StepStatus::Verified
            {
                p.status = StepStatus::DoneUnverified;
            }
            Ok(ticket)
        })
    }
    pub fn finish_verification(
        &mut self,
        ticket: &VerificationTicket,
        exit_code: Option<i32>,
        timed_out: bool,
        interrupted: bool,
        output_digest: Option<&str>,
    ) -> Result<()> {
        self.finish_verification_with_output(
            ticket,
            exit_code,
            timed_out,
            interrupted,
            output_digest,
            None,
        )
    }
    /// Capture bytes privately before calling; this transaction binds the reference
    /// and result together. Older callers may retain digest-only evidence.
    pub fn finish_verification_with_output(
        &mut self,
        ticket: &VerificationTicket,
        exit_code: Option<i32>,
        timed_out: bool,
        interrupted: bool,
        output_digest: Option<&str>,
        output_path: Option<&str>,
    ) -> Result<()> {
        let timestamp_ms = observed_ms()?;
        self.update(|s| {
            let step = revision_step(s, &ticket.revision, &ticket.step_id)?.clone();
            let e = execution(s)?;
            ensure(e.revision == ticket.revision, "stale verification revision")?;
            let v = e
                .evidence
                .iter_mut()
                .find(|v| v.id == ticket.id)
                .ok_or("unknown verification")?;
            ensure(
                v.status == VerificationStatus::Running
                    && v.step_id == ticket.step_id
                    && v.check_id == ticket.check_id
                    && v.command == ticket.command,
                "stale or altered verification ticket",
            )?;
            v.exit_code = exit_code;
            v.timed_out = timed_out;
            v.interrupted = interrupted;
            v.output_digest = output_digest.map(str::to_owned);
            v.timestamp_ms = Some(timestamp_ms);
            v.output_path = output_path.map(str::to_owned);
            v.status = if exit_code == Some(0) && !timed_out && !interrupted {
                VerificationStatus::Passed
            } else {
                VerificationStatus::Failed
            };
            let passed = checks_pass(e, &step);
            if let Some(p) = e.steps.iter_mut().find(|p| p.id == ticket.step_id)
                && p.status == StepStatus::DoneUnverified
                && passed
            {
                p.status = StepStatus::Verified;
            }
            Ok(())
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub fn record_configured_check(
        &mut self,
        revision: &str,
        check_id: &str,
        command: &str,
        exit_code: Option<i32>,
        timed_out: bool,
        interrupted: bool,
        output_digest: Option<&str>,
    ) -> Result<()> {
        self.record_configured_check_with_output(
            revision,
            check_id,
            command,
            exit_code,
            timed_out,
            interrupted,
            output_digest,
            None,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn record_configured_check_with_output(
        &mut self,
        revision: &str,
        check_id: &str,
        command: &str,
        exit_code: Option<i32>,
        timed_out: bool,
        interrupted: bool,
        output_digest: Option<&str>,
        output_path: Option<&str>,
    ) -> Result<()> {
        let timestamp_ms = observed_ms()?;
        self.update(|s| {
            let e = active_execution(s, revision)?;
            e.evidence.push(Verification {
                id: format!("v{}", e.evidence.len() + 1),
                revision: revision.into(),
                step_id: String::new(),
                check_id: check_id.into(),
                kind: VerificationKind::Configured,
                valid: true,
                command: command.into(),
                status: if exit_code == Some(0) && !timed_out && !interrupted {
                    VerificationStatus::Passed
                } else {
                    VerificationStatus::Failed
                },
                exit_code,
                timed_out,
                interrupted,
                output_digest: output_digest.map(str::to_owned),
                timestamp_ms: Some(timestamp_ms),
                output_path: output_path.map(str::to_owned),
            });
            Ok(())
        })
    }
    /// Called by the host after writes/commands/undo that may change checked work.
    /// Historical results remain visible; they no longer prove current validity.
    pub fn invalidate_verification(&mut self, reason: &str) -> Result<()> {
        text(reason, false)?;
        self.refresh()?;
        if self
            .saved
            .execution
            .as_ref()
            .is_none_or(|e| terminal(e.status))
        {
            return Ok(());
        }
        self.update(|s| {
            if let Some(e) = &mut s.execution {
                if terminal(e.status) {
                    return Ok(());
                }
                for v in &mut e.evidence {
                    if matches!(
                        v.status,
                        VerificationStatus::Passed | VerificationStatus::Running
                    ) {
                        v.valid = false;
                    }
                }
                for p in &mut e.steps {
                    if p.status == StepStatus::Verified {
                        p.status = StepStatus::DoneUnverified;
                    }
                }
                e.reason = Some(reason.into());
            }
            Ok(())
        })
    }
    pub fn record_question(&mut self, revision: &str, id: &str, question: &str) -> Result<()> {
        self.update(|s| {
            ensure(
                s.current().is_some_and(|r| r.id == revision),
                "stale question revision",
            )?;
            ensure(
                !s.questions.iter().any(|q| q.id == id),
                "question id already exists",
            )?;
            s.questions.push(Question {
                id: id.into(),
                revision: revision.into(),
                question: question.into(),
                answer: None,
            });
            if let Some(e) = &mut s.execution
                && e.status == ExecutionStatus::Active
            {
                e.status = ExecutionStatus::Waiting;
            }
            Ok(())
        })
    }
    pub fn answer_question(&mut self, revision: &str, id: &str, answer: &str) -> Result<()> {
        text(answer, true)?;
        self.update(|s| {
            ensure(
                s.current().is_some_and(|r| r.id == revision),
                "stale question answer",
            )?;
            let q = s
                .questions
                .iter_mut()
                .find(|q| q.id == id && q.revision == revision)
                .ok_or("unknown question")?;
            ensure(q.answer.is_none(), "question already answered")?;
            q.answer = Some(answer.into());
            Ok(())
        })
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn ensure(ok: bool, message: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(message.into()) }
}
fn text(s: &str, nonempty: bool) -> Result<()> {
    ensure(
        s.len() <= MAX_TEXT && (!nonempty || !s.trim().is_empty()),
        "invalid or oversized plan text",
    )
}
fn identifier(s: &str) -> Result<()> {
    ensure(
        !s.is_empty()
            && s.len() <= 128
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b)),
        "invalid plan identifier",
    )
}
fn digest(draft: &Draft) -> Result<String> {
    Ok(declass_fs::sha256_hex(
        &serde_json::to_vec(draft).map_err(err)?,
    ))
}
fn terminal(s: ExecutionStatus) -> bool {
    matches!(
        s,
        ExecutionStatus::Completed
            | ExecutionStatus::Exhausted
            | ExecutionStatus::Cancelled
            | ExecutionStatus::Superseded
    )
}
fn current<'a>(s: &'a PlanState, revision: &str, hash: &str) -> Result<&'a Revision> {
    s.current()
        .filter(|r| r.id == revision && r.digest == hash)
        .ok_or_else(|| "the plan revision changed; review it again".into())
}
fn approved(s: &PlanState, revision: &str, hash: &str) -> Result<()> {
    ensure(
        s.approval
            .as_ref()
            .is_some_and(|a| a.revision == revision && a.digest == hash),
        "approve the current plan revision first",
    )
}
fn unanswered(s: &PlanState, revision: &str) -> Result<()> {
    ensure(
        !s.questions
            .iter()
            .any(|q| q.revision == revision && q.answer.is_none()),
        "answer the plan's pending questions first",
    )
}
fn execution(s: &mut PlanState) -> Result<&mut Execution> {
    s.execution
        .as_mut()
        .ok_or_else(|| "no plan execution".into())
}
fn active_execution<'a>(s: &'a mut PlanState, revision: &str) -> Result<&'a mut Execution> {
    let r = s.current().ok_or("no plan")?;
    ensure(r.id == revision, "stale plan revision")?;
    approved(s, &r.id, &r.digest)?;
    let e = execution(s)?;
    ensure(
        e.revision == revision && e.status == ExecutionStatus::Active,
        "plan execution is not active",
    )?;
    Ok(e)
}
fn revision_step<'a>(s: &'a PlanState, revision: &str, step: &str) -> Result<&'a Step> {
    s.current()
        .filter(|r| r.id == revision)
        .and_then(|r| r.draft.steps.iter().find(|x| x.id == step))
        .ok_or_else(|| "unknown current plan step".into())
}
fn checks_pass(e: &Execution, step: &Step) -> bool {
    !step.checks.is_empty()
        && step.checks.iter().all(|c| {
            e.evidence
                .iter()
                .rev()
                .find(|v| {
                    v.kind == VerificationKind::Proposed
                        && v.step_id == step.id
                        && v.check_id == c.id
                })
                .is_some_and(|v| v.status == VerificationStatus::Passed && v.valid)
        })
}
fn ready(s: &PlanState, e: &Execution) -> bool {
    let Some(r) = s
        .revisions
        .iter()
        .find(|r| r.id == e.revision && r.digest == e.digest)
    else {
        return false;
    };
    !e.evidence
        .iter()
        .any(|v| v.status == VerificationStatus::Running)
        && r.draft.steps.iter().all(|step| {
            e.steps.iter().find(|p| p.id == step.id).is_some_and(|p| {
                if step.checks.is_empty() {
                    p.status == StepStatus::DoneUnverified
                } else {
                    p.status == StepStatus::Verified && checks_pass(e, step)
                }
            })
        })
}
/// Existing IDs are retained; additions receive host-generated IDs. IDs never
/// authorize tool access, but stable identity avoids approving the wrong step.
fn assign_ids(s: &PlanState, d: &mut Draft) -> Result<()> {
    let existing = s.current().map(|r| &r.draft.steps);
    let mut used: std::collections::HashSet<String> = s
        .revisions
        .iter()
        .flat_map(|r| r.draft.steps.iter().map(|step| step.id.clone()))
        .collect();
    let mut next = 1;
    for step in &mut d.steps {
        if step.id.is_empty() {
            loop {
                let id = format!("s{next}");
                next += 1;
                if used.insert(id.clone()) {
                    step.id = id;
                    break;
                }
            }
        } else {
            ensure(
                existing.is_some_and(|steps| steps.iter().any(|old| old.id == step.id)),
                "new steps must leave their ID empty for the host",
            )?;
        }
        let old = existing.and_then(|steps| steps.iter().find(|old| old.id == step.id));
        let mut used: std::collections::HashSet<String> = s
            .revisions
            .iter()
            .flat_map(|r| {
                r.draft
                    .steps
                    .iter()
                    .filter(|old| old.id == step.id)
                    .flat_map(|old| old.checks.iter().map(|c| c.id.clone()))
            })
            .collect();
        let mut next = 1;
        for check in &mut step.checks {
            if check.id.is_empty() {
                loop {
                    let id = format!("c{next}");
                    next += 1;
                    if used.insert(id.clone()) {
                        check.id = id;
                        break;
                    }
                }
            } else {
                ensure(
                    old.is_some_and(|old| old.checks.iter().any(|c| c.id == check.id)),
                    "new checks must leave their ID empty for the host",
                )?;
            }
        }
    }
    Ok(())
}
fn validate_draft(d: &Draft) -> Result<()> {
    text(&d.title, true)?;
    text(&d.objective, true)?;
    for list in [&d.scope, &d.non_goals, &d.assumptions, &d.decisions] {
        ensure(list.len() <= 100, "too many plan sections")?;
        for t in list {
            text(t, true)?;
        }
    }
    ensure(
        !d.steps.is_empty() && d.steps.len() <= MAX_STEPS,
        "a plan requires 1..100 steps",
    )?;
    let mut ids = std::collections::HashSet::new();
    for step in &d.steps {
        identifier(&step.id)?;
        ensure(ids.insert(&step.id), "duplicate step id")?;
        text(&step.title, true)?;
        text(&step.description, false)?;
        ensure(
            step.paths.len() <= 100 && step.checks.len() <= 50 && step.acceptance.len() <= 100,
            "too many step paths or checks",
        )?;
        for criterion in &step.acceptance {
            text(criterion, true)?;
        }
        for p in &step.paths {
            text(p, true)?;
            ensure(
                Path::new(p)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
                "plan path must be relative without traversal",
            )?;
        }
        let mut check_ids = std::collections::HashSet::new();
        for c in &step.checks {
            identifier(&c.id)?;
            ensure(check_ids.insert(&c.id), "duplicate check id")?;
            text(&c.command, true)?;
            text(&c.description, false)?;
        }
    }
    Ok(())
}
fn validate(s: &PlanState) -> Result<()> {
    ensure(s.version == 1, "unsupported plan schema")?;
    ensure(
        s.revisions.len() <= MAX_REVISIONS
            && s.execution_history.len() <= MAX_REVISIONS
            && s.questions.len() <= 1000,
        "plan history limit exceeded",
    )?;
    for (i, r) in s.revisions.iter().enumerate() {
        ensure(r.id == format!("r{}", i + 1), "invalid revision sequence")?;
        validate_draft(&r.draft)?;
        ensure(
            r.digest == digest(&r.draft)?,
            "plan revision digest mismatch",
        )?;
    }
    if let Some(a) = &s.approval {
        current(s, &a.revision, &a.digest)?;
    }
    if let Some(b) = &s.carried_budget {
        ensure(
            b.turns <= b.max_turns && (1..=1000).contains(&b.max_turns) && s.execution.is_none(),
            "invalid carried turn budget",
        )?;
    }
    let mut execution_revisions = std::collections::HashSet::new();
    for e in s.execution.iter().chain(&s.execution_history) {
        ensure(
            execution_revisions.insert(&e.revision),
            "duplicate execution revision",
        )?;
        let r = s
            .revisions
            .iter()
            .find(|r| r.id == e.revision && r.digest == e.digest)
            .ok_or("execution revision mismatch")?;
        ensure(
            (1..=1000).contains(&e.max_turns)
                && e.turns <= e.max_turns
                && (!e.turn_open || e.turns > 0),
            "invalid execution turn budget",
        )?;
        ensure(
            e.steps.len() == r.draft.steps.len() && e.evidence.len() <= MAX_EVIDENCE,
            "invalid execution size",
        )?;
        if let Some(reason) = &e.reason {
            text(reason, false)?;
        }
        for (p, step) in e.steps.iter().zip(&r.draft.steps) {
            ensure(p.id == step.id, "step progress does not match revision")?;
            if let Some(n) = &p.note {
                text(n, false)?;
            }
            if p.status == StepStatus::Verified {
                ensure(
                    checks_pass(e, step),
                    "step verification has no passing host evidence",
                )?;
            }
        }
        for (i, v) in e.evidence.iter().enumerate() {
            ensure(
                v.id == format!("v{}", i + 1) && v.revision == e.revision,
                "invalid verification identity",
            )?;
            identifier(&v.check_id)?;
            text(&v.command, true)?;
            if let Some(hash) = &v.output_digest {
                ensure(
                    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
                    "invalid output digest",
                )?;
            }
            match v.kind {
                VerificationKind::Proposed => {
                    let c = r
                        .draft
                        .steps
                        .iter()
                        .find(|step| step.id == v.step_id)
                        .and_then(|step| step.checks.iter().find(|c| c.id == v.check_id))
                        .ok_or("verification has unknown check")?;
                    ensure(
                        c.command == v.command,
                        "verification command differs from approved plan",
                    )?;
                }
                VerificationKind::Configured => ensure(
                    v.step_id.is_empty(),
                    "configured checks cannot substitute for step checks",
                )?,
            }
            if let Some(path) = &v.output_path {
                output_reference(path)?;
                ensure(
                    v.output_digest.is_some() && v.timestamp_ms.is_some(),
                    "captured output requires a digest and observation time",
                )?;
            }
            if v.status == VerificationStatus::Running {
                ensure(
                    v.timestamp_ms.is_none() && v.output_path.is_none(),
                    "running checks cannot have completed evidence",
                )?;
            }
            if v.status == VerificationStatus::Passed {
                ensure(
                    v.exit_code == Some(0) && !v.timed_out && !v.interrupted,
                    "invalid successful check evidence",
                )?;
            }
            if v.status == VerificationStatus::Running {
                ensure(
                    e.turn_open && v.exit_code.is_none() && !v.timed_out && !v.interrupted,
                    "invalid running check",
                )?;
            }
        }
        if e.status == ExecutionStatus::Completed {
            ensure(
                !e.turn_open && ready(s, e),
                "completed execution has unfinished steps",
            )?;
        }
        if e.status == ExecutionStatus::Exhausted {
            ensure(e.turns == e.max_turns, "invalid exhausted state")?;
        }
    }
    if let Some(e) = &s.execution {
        current(s, &e.revision, &e.digest)?;
        if e.status == ExecutionStatus::Active {
            approved(s, &e.revision, &e.digest)?;
            unanswered(s, &e.revision)?;
        }
    }
    ensure(
        s.execution_history
            .iter()
            .all(|e| terminal(e.status) && !e.turn_open),
        "invalid historical execution",
    )?;
    // Revisions may be edited repeatedly, but an unfinished execution's debit
    // and ceiling cannot be erased or enlarged by moving it to history.
    let mut previous: Option<&Execution> = None;
    let mut previous_revision = 0;
    for e in s.execution_history.iter().chain(&s.execution) {
        let index = s
            .revisions
            .iter()
            .position(|r| r.id == e.revision)
            .ok_or("unknown execution revision")?
            + 1;
        ensure(
            index > previous_revision,
            "execution history is not chronological",
        )?;
        if let Some(old) = previous
            && matches!(
                old.status,
                ExecutionStatus::Superseded | ExecutionStatus::Exhausted
            )
        {
            ensure(
                e.turns >= old.turns && e.max_turns == old.max_turns,
                "unfinished execution budget was reset",
            )?;
        }
        previous = Some(e);
        previous_revision = index;
    }
    if let Some(b) = &s.carried_budget {
        ensure(
            s.execution_history.last().is_some_and(|e| {
                matches!(
                    e.status,
                    ExecutionStatus::Superseded | ExecutionStatus::Exhausted
                ) && b.turns == e.turns
                    && b.max_turns == e.max_turns
            }),
            "carried budget does not match execution history",
        )?;
    } else if s.execution.is_none()
        && s.execution_history.last().is_some_and(|e| {
            matches!(
                e.status,
                ExecutionStatus::Superseded | ExecutionStatus::Exhausted
            )
        })
    {
        return Err("unfinished execution is missing its carried budget".into());
    }
    let mut question_ids = std::collections::HashSet::new();
    for q in &s.questions {
        identifier(&q.id)?;
        ensure(question_ids.insert(&q.id), "duplicate question")?;
        ensure(
            s.revisions.iter().any(|r| r.id == q.revision),
            "question references unknown revision",
        )?;
        text(&q.question, true)?;
        if let Some(a) = &q.answer {
            text(a, true)?;
        }
    }
    Ok(())
}
fn observed_ms() -> Result<u64> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "host clock predates the Unix epoch")?;
    u64::try_from(elapsed.as_millis()).map_err(|_| "host timestamp exceeds supported range".into())
}
fn output_reference(path: &str) -> Result<()> {
    ensure(
        path.len() <= 4096 && !path.contains(['\\', '\0']),
        "invalid verification output path",
    )?;
    let pieces: Vec<_> = path.split('/').collect();
    ensure(
        pieces.len() >= 2
            && pieces[0] == "plan-checks"
            && pieces
                .iter()
                .all(|p| !p.is_empty() && *p != "." && *p != ".."),
        "verification output must be below plan-checks without traversal",
    )?;
    ensure(
        Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_))),
        "invalid verification output path",
    )
}
fn regular_or_missing(file: &PinnedParent) -> Result<()> {
    if file.file_type().map_err(err)?.is_some() {
        let f = file.open_read().map_err(err)?;
        ensure(
            f.metadata().map_err(err)?.is_file(),
            "plan state must be a regular file",
        )?;
    }
    Ok(())
}
fn read(file: &PinnedParent) -> Result<PlanState> {
    if file.file_type().map_err(err)?.is_none() {
        return Ok(PlanState::default());
    }
    let f = file.open_read().map_err(err)?;
    ensure(
        f.metadata().map_err(err)?.is_file(),
        "plan state must be a regular file",
    )?;
    let mut bytes = Vec::new();
    f.take((MAX_FILE + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    ensure(bytes.len() <= MAX_FILE, "plan state exceeds 4 MiB")?;
    let s: PlanState = serde_json::from_slice(&bytes)
        .map_err(|_| "invalid plan state; no approval or execution allowed".to_owned())?;
    validate(&s)?;
    Ok(s)
}
struct Lease(File);
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
fn acquire(pin: &PinnedParent) -> Result<Lease> {
    if pin.file_type().map_err(err)?.is_none() {
        match pin.create_temp(std::ffi::OsStr::new("plan.lock"), 0o600) {
            Ok(file) => {
                file.sync_all().map_err(err)?;
                pin.sync().map_err(err)?;
            }
            Err(e) => {
                if pin.file_type().map_err(err)?.is_none() {
                    return Err(err(e));
                }
            }
        }
    }
    let file = pin.open_read().map_err(err)?;
    ensure(
        file.metadata().map_err(err)?.is_file(),
        "plan lock must be regular",
    )?;
    file.try_lock()
        .map_err(|_| "plan is being updated by another writer".to_owned())?;
    Ok(Lease(file))
}
fn markdown(s: &PlanState) -> String {
    let Some(r) = s.current() else {
        return "# No saved plan\n".into();
    };
    let mut out = format!(
        "# {}\n\nRevision: {}\nDigest: {}\n\n{}\n\n",
        r.draft.title, r.id, r.digest, r.draft.objective
    );
    for (name, items) in [
        ("Scope", &r.draft.scope),
        ("Out of scope", &r.draft.non_goals),
        ("Assumptions", &r.draft.assumptions),
        ("Decisions", &r.draft.decisions),
    ] {
        if !items.is_empty() {
            out.push_str(&format!("## {name}\n\n"));
            for item in items {
                out.push_str(&format!("- {item}\n"));
            }
            out.push('\n');
        }
    }
    out.push_str("## Steps\n\n");
    for step in &r.draft.steps {
        let progress = s
            .execution
            .as_ref()
            .and_then(|e| e.steps.iter().find(|p| p.id == step.id));
        out.push_str(&format!(
            "### {}: {}\n\n{}\n\nStatus: {:?}\n",
            step.id,
            step.title,
            step.description,
            progress.map_or(StepStatus::Pending, |p| p.status)
        ));
        for path in &step.paths {
            out.push_str(&format!("- Path: {path}\n"));
        }
        for criterion in &step.acceptance {
            out.push_str(&format!("- Acceptance: {criterion}\n"));
        }
        for check in &step.checks {
            out.push_str(&format!(
                "- Check {}: {}\n  Command: {}\n",
                check.id, check.description, check.command
            ));
        }
        out.push('\n');
    }
    out.push_str("This Markdown is a private derived view. Edit and approve the canonical plan through Declass.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn draft(checked: bool) -> Draft {
        Draft {
            title: "A private plan".into(),
            objective: "Implement the requested change".into(),
            steps: vec![Step {
                title: "Implement".into(),
                acceptance: vec!["Requested behavior works".into()],
                checks: if checked {
                    vec![Check {
                        command: "test -f result.txt".into(),
                        description: "Check output".into(),
                        ..Check::default()
                    }]
                } else {
                    vec![]
                },
                ..Step::default()
            }],
            ..Draft::default()
        }
    }
    fn fixture() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::load(&dir.path().canonicalize().unwrap()).unwrap();
        (dir, store)
    }
    fn implemented(store: &mut Store, checked: bool, turns: u64) -> Revision {
        let r = store.revise(None, draft(checked)).unwrap();
        store.implement(&r.id, &r.digest, turns).unwrap();
        r
    }
    #[test]
    fn missing_legacy_state_is_read_only_and_approval_is_not_execution() {
        let (dir, mut s) = fixture();
        assert!(!s.recover().unwrap());
        s.invalidate_verification("nothing changed").unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        let r = s.revise(None, draft(false)).unwrap();
        assert_eq!(r.id, "r1");
        assert_eq!(r.draft.steps[0].id, "s1");
        s.approve(&r.id, &r.digest).unwrap();
        assert!(s.snapshot().execution.is_none());
        assert!(s.prompt().is_none());
        assert!(s.begin_turn().is_err());
        assert!(dir.path().join("plan.md").is_file());
        assert_eq!(
            std::fs::metadata(dir.path().join("plan.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    #[test]
    fn revisions_are_immutable_and_stale_editor_or_approval_cannot_execute() {
        let (dir, mut s) = fixture();
        let r1 = s.revise(None, draft(true)).unwrap();
        s.approve(&r1.id, &r1.digest).unwrap();
        let mut stale = Store::load(&dir.path().canonicalize().unwrap()).unwrap();
        let mut changed = r1.draft.clone();
        changed.objective.push_str(" after review");
        changed.steps.push(Step {
            title: "Document".into(),
            ..Step::default()
        });
        let r2 = s.revise(Some(&r1.id), changed).unwrap();
        assert_eq!(s.snapshot().revisions[0], r1);
        assert_eq!(r2.draft.steps[0].id, "s1");
        assert_eq!(r2.draft.steps[1].id, "s2");
        assert!(s.snapshot().approval.is_none());
        assert!(stale.approve(&r1.id, &r1.digest).is_err());
        assert!(stale.revise(Some(&r1.id), r1.draft.clone()).is_err());
        assert!(s.implement(&r2.id, &r1.digest, 20).is_err());
        assert!(s.implement(&r1.id, &r1.digest, 20).is_err());
    }
    #[test]
    fn interrupted_turn_is_charged_and_editing_cannot_reset_allowance() {
        let (dir, mut s) = fixture();
        let r = implemented(&mut s, false, 2);
        s.begin_turn().unwrap();
        assert!(s.begin_turn().is_err());
        let mut reopened = Store::load(&dir.path().canonicalize().unwrap()).unwrap();
        assert!(reopened.active());
        assert!(reopened.recover().unwrap());
        assert!(!reopened.active());
        assert_eq!(reopened.snapshot().execution.as_ref().unwrap().turns, 1);
        let mut revised = r.draft.clone();
        revised.title.push_str(" revised");
        let r2 = reopened.revise(Some(&r.id), revised).unwrap();
        reopened.implement(&r2.id, &r2.digest, 100).unwrap();
        let e = reopened.snapshot().execution.as_ref().unwrap();
        assert_eq!((e.turns, e.max_turns), (1, 2));
        reopened.begin_turn().unwrap();
        reopened
            .finish_turn(&crate::TurnEnd::Replied {
                message: "working".into(),
            })
            .unwrap();
        assert_eq!(
            reopened.snapshot().execution.as_ref().unwrap().status,
            ExecutionStatus::Exhausted
        );
        assert!(reopened.resume(&r2.id, &r2.digest).is_err());
    }
    #[test]
    fn unknown_checks_are_not_passes_and_cannot_be_retroactively_settled() {
        let (dir, mut s) = fixture();
        let r = implemented(&mut s, true, 20);
        s.begin_turn().unwrap();
        let t = s.begin_verification(&r.id, "s1", "c1").unwrap();
        assert_eq!(t.command, "test -f result.txt");
        let mut reopened = Store::load(&dir.path().canonicalize().unwrap()).unwrap();
        reopened.recover().unwrap();
        assert_eq!(
            reopened.snapshot().execution.as_ref().unwrap().evidence[0].status,
            VerificationStatus::Unknown
        );
        assert!(
            reopened
                .finish_verification(&t, Some(0), false, false, None)
                .is_err()
        );
        assert!(!reopened.ready_to_finish());
    }
    #[test]
    fn completed_evidence_binds_host_time_and_private_relative_output() {
        let (dir, mut s) = fixture();
        let r = implemented(&mut s, true, 20);
        s.begin_turn().unwrap();
        let ticket = s.begin_verification(&r.id, "s1", "c1").unwrap();
        let prior = std::fs::read(dir.path().join("plan.json")).unwrap();
        let digest = declass_fs::sha256_hex(b"actual host output");
        for bad in [
            "/plan-checks/o",
            "other/o",
            "plan-checks",
            "plan-checks/../plan.json",
            "plan-checks/./o",
            "plan-checks//o",
            "plan-checks/o/",
            "plan-checks/..\\o",
        ] {
            assert!(
                s.finish_verification_with_output(
                    &ticket,
                    Some(0),
                    false,
                    false,
                    Some(&digest),
                    Some(bad)
                )
                .is_err(),
                "{bad}"
            );
            assert_eq!(std::fs::read(dir.path().join("plan.json")).unwrap(), prior);
        }
        assert!(
            s.finish_verification_with_output(
                &ticket,
                Some(0),
                false,
                false,
                None,
                Some("plan-checks/r1-v1.txt")
            )
            .is_err()
        );
        let before = observed_ms().unwrap();
        s.finish_verification_with_output(
            &ticket,
            Some(0),
            false,
            false,
            Some(&digest),
            Some("plan-checks/r1-v1.txt"),
        )
        .unwrap();
        let after = observed_ms().unwrap();
        let saved = Store::load(&dir.path().canonicalize().unwrap()).unwrap();
        let evidence = &saved.snapshot().execution.as_ref().unwrap().evidence[0];
        assert!(matches!(evidence.timestamp_ms, Some(ms) if ms >= before && ms <= after));
        assert_eq!(
            evidence.output_path.as_deref(),
            Some("plan-checks/r1-v1.txt")
        );
        assert_eq!(evidence.output_digest.as_deref(), Some(digest.as_str()));
        s.record_configured_check_with_output(
            &r.id,
            "owner1",
            "false",
            Some(1),
            false,
            false,
            Some(&digest),
            Some("plan-checks/r1-owner1.txt"),
        )
        .unwrap();
        let evidence = &s.snapshot().execution.as_ref().unwrap().evidence[1];
        assert_eq!(evidence.status, VerificationStatus::Failed);
        assert!(evidence.timestamp_ms.is_some());
        assert_eq!(
            evidence.output_path.as_deref(),
            Some("plan-checks/r1-owner1.txt")
        );
        s.invalidate_verification("workspace changed").unwrap();
        assert_eq!(
            s.snapshot().execution.as_ref().unwrap().evidence[0].timestamp_ms,
            saved.snapshot().execution.as_ref().unwrap().evidence[0].timestamp_ms
        );
    }
    #[test]
    fn older_digest_only_evidence_loads_without_inventing_time_or_capture() {
        let (dir, mut s) = fixture();
        let r = implemented(&mut s, true, 20);
        s.begin_turn().unwrap();
        let ticket = s.begin_verification(&r.id, "s1", "c1").unwrap();
        s.finish_verification(&ticket, Some(0), false, false, None)
            .unwrap();
        let path = dir.path().join("plan.json");
        let mut raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let evidence = raw["execution"]["evidence"][0].as_object_mut().unwrap();
        evidence.remove("timestamp_ms");
        evidence.remove("output_path");
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        let loaded = Store::load(&dir.path().canonicalize().unwrap()).unwrap();
        let evidence = &loaded.snapshot().execution.as_ref().unwrap().evidence[0];
        assert!(evidence.timestamp_ms.is_none() && evidence.output_path.is_none());
    }
    #[test]
    fn only_actual_latest_success_verifies_and_history_survives_invalidation() {
        let (_dir, mut s) = fixture();
        let r = implemented(&mut s, true, 20);
        s.begin_turn().unwrap();
        assert!(
            s.report_step(&r.id, "s1", StepStatus::Verified, None)
                .is_err()
        );
        s.report_step(&r.id, "s1", StepStatus::DoneUnverified, None)
            .unwrap();
        let t = s.begin_verification(&r.id, "s1", "c1").unwrap();
        s.finish_verification(&t, Some(0), true, false, None)
            .unwrap();
        assert!(!s.ready_to_finish());
        let t = s.begin_verification(&r.id, "s1", "c1").unwrap();
        s.finish_verification(&t, Some(0), false, false, None)
            .unwrap();
        assert!(s.ready_to_finish());
        s.invalidate_verification("workspace changed").unwrap();
        assert!(!s.ready_to_finish());
        let e = s.snapshot().execution.as_ref().unwrap();
        assert_eq!(e.evidence[0].status, VerificationStatus::Failed);
        assert_eq!(e.evidence[1].status, VerificationStatus::Passed);
        assert!(!e.evidence[1].valid);
        s.report_step(&r.id, "s1", StepStatus::DoneUnverified, None)
            .unwrap();
        assert!(!s.ready_to_finish());
        let t = s.begin_verification(&r.id, "s1", "c1").unwrap();
        s.finish_verification(&t, Some(0), false, false, None)
            .unwrap();
        s.finish_turn(&crate::TurnEnd::Completed {
            summary: "done".into(),
        })
        .unwrap();
        s.invalidate_verification("unrelated later work").unwrap();
        assert_eq!(
            s.snapshot().execution.as_ref().unwrap().status,
            ExecutionStatus::Completed
        );
    }
    #[test]
    fn configured_checks_do_not_substitute_for_plan_checks_and_unchecked_is_honest() {
        let (_dir, mut s) = fixture();
        let r = implemented(&mut s, true, 20);
        s.begin_turn().unwrap();
        s.report_step(&r.id, "s1", StepStatus::DoneUnverified, None)
            .unwrap();
        s.record_configured_check(&r.id, "owner1", "true", Some(0), false, false, None)
            .unwrap();
        assert!(!s.ready_to_finish());
        let (_dir, mut s) = fixture();
        let r = implemented(&mut s, false, 20);
        s.begin_turn().unwrap();
        s.report_step(&r.id, "s1", StepStatus::DoneUnverified, None)
            .unwrap();
        assert!(s.ready_to_finish());
        s.finish_turn(&crate::TurnEnd::Completed {
            summary: "done without checks".into(),
        })
        .unwrap();
        assert_eq!(
            s.snapshot().execution.as_ref().unwrap().steps[0].status,
            StepStatus::DoneUnverified
        );
    }
    #[test]
    fn questions_are_revision_bound_and_answers_never_start_execution() {
        let (_dir, mut s) = fixture();
        let r = s.revise(None, draft(false)).unwrap();
        s.record_question(&r.id, "q1", "Which approach?").unwrap();
        assert!(s.approve(&r.id, &r.digest).is_err());
        assert!(s.answer_question("r2", "q1", "first").is_err());
        s.answer_question(&r.id, "q1", "first").unwrap();
        assert!(s.snapshot().approval.is_none());
        assert!(s.answer_question(&r.id, "q1", "second").is_err());
        s.implement(&r.id, &r.digest, 20).unwrap();
        s.begin_turn().unwrap();
        s.record_question(&r.id, "q2", "Need more input").unwrap();
        s.finish_turn(&crate::TurnEnd::Replied {
            message: "waiting".into(),
        })
        .unwrap();
        assert!(s.resume(&r.id, &r.digest).is_err());
        s.answer_question(&r.id, "q2", "continue").unwrap();
        s.resume(&r.id, &r.digest).unwrap();
    }
    #[test]
    fn questions_cannot_revive_paused_or_exhausted_execution() {
        let (_dir, mut s) = fixture();
        let r = implemented(&mut s, false, 1);
        s.pause("Explicit operator pause").unwrap();
        s.record_question(&r.id, "ordinary1", "An unrelated question?")
            .unwrap();
        assert_eq!(
            s.snapshot().execution.as_ref().unwrap().status,
            ExecutionStatus::Paused
        );
        s.answer_question(&r.id, "ordinary1", "An answer, not resume")
            .unwrap();
        assert!(!s.active());
        assert_eq!(s.snapshot().execution.as_ref().unwrap().turns, 0);
        s.resume(&r.id, &r.digest).unwrap();
        s.begin_turn().unwrap();
        s.finish_turn(&crate::TurnEnd::Replied {
            message: "worked".into(),
        })
        .unwrap();
        assert_eq!(
            s.snapshot().execution.as_ref().unwrap().status,
            ExecutionStatus::Exhausted
        );
        s.record_question(&r.id, "ordinary2", "A later question?")
            .unwrap();
        s.answer_question(&r.id, "ordinary2", "Still no turn allowance")
            .unwrap();
        assert_eq!(
            s.snapshot().execution.as_ref().unwrap().status,
            ExecutionStatus::Exhausted
        );
        assert!(!s.active());
        assert!(s.resume(&r.id, &r.digest).is_err());
    }
    #[test]
    fn malformed_unknown_oversized_and_digest_mismatch_fail_closed() {
        for content in [b"{".as_slice(), b"{\"version\":99}".as_slice()] {
            let (dir, _s) = fixture();
            std::fs::write(dir.path().join("plan.json"), content).unwrap();
            assert!(Store::load(&dir.path().canonicalize().unwrap()).is_err());
        }
        let (dir, mut s) = fixture();
        s.revise(None, draft(false)).unwrap();
        let mut v = serde_json::to_value(s.snapshot()).unwrap();
        v["unknown"] = serde_json::json!(true);
        std::fs::write(
            dir.path().join("plan.json"),
            serde_json::to_vec(&v).unwrap(),
        )
        .unwrap();
        assert!(Store::load(&dir.path().canonicalize().unwrap()).is_err());
        v.as_object_mut().unwrap().remove("unknown");
        v["revisions"][0]["draft"]["title"] = serde_json::json!("tampered");
        std::fs::write(
            dir.path().join("plan.json"),
            serde_json::to_vec(&v).unwrap(),
        )
        .unwrap();
        assert!(Store::load(&dir.path().canonicalize().unwrap()).is_err());
        std::fs::write(dir.path().join("plan.json"), vec![b' '; MAX_FILE + 1]).unwrap();
        assert!(Store::load(&dir.path().canonicalize().unwrap()).is_err());
    }
    #[test]
    fn active_writer_lock_and_leaf_symlinks_are_refused() {
        let (dir, mut s) = fixture();
        let lease = acquire(&s.lock).unwrap();
        assert!(s.revise(None, draft(false)).is_err());
        drop(lease);
        let outside = dir.path().join("outside");
        std::fs::write(&outside, "untouched").unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("plan.json")).unwrap();
        assert!(s.revise(None, draft(false)).is_err());
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "untouched");
    }
    #[test]
    fn projection_failure_does_not_undo_canonical_save_or_follow_link() {
        let (dir, mut s) = fixture();
        let outside = dir.path().join("outside");
        std::fs::write(&outside, "untouched").unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("plan.md")).unwrap();
        let r = s.revise(None, draft(false)).unwrap();
        assert!(s.projection_error().is_some());
        let loaded = Store::load(&dir.path().canonicalize().unwrap()).unwrap();
        assert_eq!(loaded.current(), Some(&r));
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "untouched");
    }
    #[test]
    fn pinned_parent_rename_cannot_redirect_canonical_writes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("run")).unwrap();
        std::fs::create_dir(root.join("outside")).unwrap();
        let mut s = Store::load(&root.join("run")).unwrap();
        std::fs::rename(root.join("run"), root.join("original")).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("run")).unwrap();
        s.revise(None, draft(false)).unwrap();
        assert!(root.join("original/plan.json").exists());
        assert!(!root.join("outside/plan.json").exists());
        assert!(Store::load(&root.join("run")).is_err());
    }
    #[test]
    fn completed_history_retains_outcome_and_cannot_be_revived_by_later_question() {
        let (_dir, mut s) = fixture();
        let r = implemented(&mut s, false, 20);
        s.begin_turn().unwrap();
        s.report_step(&r.id, "s1", StepStatus::DoneUnverified, None)
            .unwrap();
        s.finish_turn(&crate::TurnEnd::Completed {
            summary: "done".into(),
        })
        .unwrap();
        s.record_question(&r.id, "later", "An unrelated later question?")
            .unwrap();
        assert_eq!(
            s.snapshot().execution.as_ref().unwrap().status,
            ExecutionStatus::Completed
        );
        let mut d = r.draft.clone();
        d.title = "Next change".into();
        let r2 = s.revise(Some(&r.id), d).unwrap();
        assert_eq!(
            s.snapshot().execution_history[0].status,
            ExecutionStatus::Completed
        );
        s.implement(&r2.id, &r2.digest, 10).unwrap();
        assert_eq!(s.snapshot().execution.as_ref().unwrap().turns, 0);
    }
    #[test]
    fn invalid_candidate_never_replaces_canonical_state_and_corrupt_budget_is_rejected() {
        let (dir, mut s) = fixture();
        let r = implemented(&mut s, false, 2);
        s.begin_turn().unwrap();
        s.finish_turn(&crate::TurnEnd::Interrupted).unwrap();
        let before = std::fs::read(dir.path().join("plan.json")).unwrap();
        let mut d = r.draft.clone();
        d.objective = "x".repeat(MAX_TEXT + 1);
        assert!(s.revise(Some(&r.id), d).is_err());
        assert_eq!(std::fs::read(dir.path().join("plan.json")).unwrap(), before);
        let mut d = r.draft.clone();
        d.title.push_str(" revised");
        s.revise(Some(&r.id), d).unwrap();
        let mut value = serde_json::to_value(s.snapshot()).unwrap();
        value["carried_budget"]["turns"] = serde_json::json!(0);
        std::fs::write(
            dir.path().join("plan.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert!(Store::load(&dir.path().canonicalize().unwrap()).is_err());
    }
}
