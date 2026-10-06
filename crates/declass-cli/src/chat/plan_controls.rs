// SPDX-License-Identifier: GPL-3.0-or-later
//! Operator-owned plan controls. Text and UI actions share revision validation;
//! neither model prose nor a saved Markdown checkbox can authorize execution.

use super::*;
use declass_agent::plans::{Draft, Store};
use declass_tui::workspace::{
    PlanAction as UiAction, PlanIdentity, PlanSnapshot, PlanStepProgress,
};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PlanAction<'a> {
    On,
    Off,
    Status,
    Task(&'a str),
    Review(Option<&'a str>),
    Edit,
    Revise(&'a str),
    Approve(&'a str),
    Implement(&'a str),
    Pause,
    Resume(&'a str),
    Invalid(&'static str),
}

impl PlanAction<'_> {
    pub(super) fn inspect_only(&self) -> bool {
        matches!(self, Self::Status | Self::Review(_) | Self::Invalid(_))
    }
}

pub(super) fn plan_action(raw: &str) -> PlanAction<'_> {
    let text = raw.trim();
    let (word, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let rest = rest.trim();
    let revision = |s: &str| {
        s.strip_prefix('r').is_some_and(|n| {
            !n.is_empty() && !n.starts_with('0') && n.bytes().all(|b| b.is_ascii_digit())
        })
    };
    match (word, rest.is_empty()) {
        ("" | "on", true) => PlanAction::On,
        ("off", true) => PlanAction::Off,
        ("status", true) => PlanAction::Status,
        ("edit", true) => PlanAction::Edit,
        ("pause", true) => PlanAction::Pause,
        ("review", true) => PlanAction::Review(None),
        ("review", false) if revision(rest) => PlanAction::Review(Some(rest)),
        ("approve", false) if revision(rest) => PlanAction::Approve(rest),
        ("implement", false) if revision(rest) => PlanAction::Implement(rest),
        ("resume", false) if revision(rest) => PlanAction::Resume(rest),
        ("revise", false) => PlanAction::Revise(rest),
        ("task", false) => PlanAction::Task(rest),
        ("review" | "approve" | "implement" | "resume", _) => PlanAction::Invalid(
            "Use /plan review [rN], /plan approve rN, /plan implement rN, or /plan resume rN.",
        ),
        ("on" | "off" | "status" | "edit" | "pause", _) => PlanAction::Invalid(
            "This plan control takes no arguments. Use /plan task <text> for a task beginning with a command word.",
        ),
        ("revise" | "task", true) => {
            PlanAction::Invalid("Describe the task or revision after the command.")
        }
        _ => PlanAction::Task(text),
    }
}

fn load(run_dir: &Path) -> Result<Store> {
    Store::load(run_dir).map_err(anyhow::Error::msg)
}

pub(super) enum Effect {
    Done,
    Prompt(String),
    Restart,
}

fn checked_identity(store: &Store, requested: &str) -> Result<PlanIdentity> {
    let current = store
        .current()
        .context("There is no saved plan. Start with /plan <task>.")?;
    ensure!(
        current.id == requested,
        "The plan changed. Review {} before continuing.",
        current.id
    );
    Ok(PlanIdentity {
        revision: current.id.clone(),
        digest: current.digest.clone(),
    })
}

fn validate_identity(store: &Store, identity: &PlanIdentity) -> Result<()> {
    let actual = checked_identity(store, &identity.revision)?;
    ensure!(
        actual.digest == identity.digest,
        "The plan changed. Open its current revision before continuing."
    );
    Ok(())
}

fn say(ctx: &TurnCtx<'_>, text: &str) {
    ctx.io
        .screen
        .line(&declass_tui::term::safe(&(ctx.shown)(text)));
}

pub(super) fn describe(run_dir: &Path) -> Result<String> {
    use declass_agent::plans::{ExecutionStatus, StepStatus};
    let store = load(run_dir)?;
    let Some(revision) = store.current() else {
        return Ok("No saved plan yet. Use /plan <task>.".into());
    };
    let mut status = if store
        .snapshot()
        .approval
        .as_ref()
        .is_some_and(|a| a.revision == revision.id && a.digest == revision.digest)
    {
        format!("{} approved", revision.id)
    } else {
        format!("{} awaiting review", revision.id)
    };
    if let Some(execution) = &store.snapshot().execution {
        let complete = execution
            .steps
            .iter()
            .filter(|s| matches!(s.status, StepStatus::DoneUnverified | StepStatus::Verified))
            .count();
        let label = match execution.status {
            ExecutionStatus::Active => "Implementing",
            ExecutionStatus::Waiting => "Waiting for your answer",
            ExecutionStatus::Paused => "Paused",
            ExecutionStatus::Completed => "Completed",
            ExecutionStatus::Exhausted => "Turn limit reached",
            ExecutionStatus::Cancelled => "Cancelled",
            ExecutionStatus::Superseded => "Superseded",
        };
        status = format!(
            "{label} {} · {complete}/{} steps · {}/{} turns",
            revision.id,
            execution.steps.len(),
            execution.turns,
            execution.max_turns
        );
        if let Some(reason) = &execution.reason {
            status.push_str(&format!("\n{reason}"));
        }
    }
    Ok(status)
}

fn local_draft(draft: &Draft, shown: &dyn Fn(&str) -> String) -> Draft {
    fn restore(value: &mut serde_json::Value, shown: &dyn Fn(&str) -> String) {
        match value {
            serde_json::Value::String(text) => *text = shown(text),
            serde_json::Value::Array(values) => values.iter_mut().for_each(|v| restore(v, shown)),
            serde_json::Value::Object(values) => {
                values.values_mut().for_each(|v| restore(v, shown))
            }
            _ => {}
        }
    }
    let mut value = serde_json::to_value(draft).expect("plan draft serializes");
    restore(&mut value, shown);
    serde_json::from_value(value).expect("restoring strings preserves the plan structure")
}

fn snapshot(ctx: &TurnCtx<'_>) -> Result<Option<PlanSnapshot>> {
    let store = load(ctx.run_dir)?;
    let Some(revision) = store.current() else {
        return Ok(None);
    };
    let execution = store.snapshot().execution.as_ref();
    let approved = store
        .snapshot()
        .approval
        .as_ref()
        .is_some_and(|a| a.revision == revision.id && a.digest == revision.digest);
    let unanswered = store
        .snapshot()
        .questions
        .iter()
        .any(|q| q.revision == revision.id && q.answer.is_none());
    let changes = store
        .snapshot()
        .revisions
        .iter()
        .rev()
        .nth(1)
        .map(|previous| {
            let old = serde_json::to_value(&previous.draft).expect("plan serializes");
            let new = serde_json::to_value(&revision.draft).expect("plan serializes");
            let mut changes = format!("{} → {}\n", previous.id, revision.id);
            for field in [
                "title",
                "objective",
                "scope",
                "non_goals",
                "assumptions",
                "decisions",
                "steps",
            ] {
                if old[field] != new[field] {
                    changes.push_str(&format!(
                        "\n{}\nBefore:\n{}\nAfter:\n{}\n",
                        field.replace('_', " "),
                        serde_json::to_string_pretty(&old[field]).unwrap(),
                        serde_json::to_string_pretty(&new[field]).unwrap()
                    ));
                }
            }
            (ctx.shown)(&changes)
        });
    let verification = execution
        .map(|e| {
            e.evidence
                .iter()
                .map(|v| {
                    let observed = v
                        .timestamp_ms
                        .and_then(|ms| {
                            time::OffsetDateTime::from_unix_timestamp_nanos(
                                i128::from(ms) * 1_000_000,
                            )
                            .ok()
                        })
                        .map(|time| time.to_string())
                        .unwrap_or_else(|| "time unavailable".into());
                    (ctx.shown)(&format!(
                        "{} / {} · {:?}{} · exit {:?} · {}\n{}{}",
                        v.step_id,
                        v.check_id,
                        v.status,
                        if v.valid { "" } else { " (stale)" },
                        v.exit_code,
                        observed,
                        v.command,
                        v.output_path
                            .as_ref()
                            .map(|p| format!("\nPrivate output: {p}"))
                            .unwrap_or_default()
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(PlanSnapshot {
        identity: PlanIdentity {
            revision: revision.id.clone(),
            digest: revision.digest.clone(),
        },
        draft: local_draft(&revision.draft, ctx.shown.as_ref()),
        location: ctx.run_dir.join("plan.md").display().to_string(),
        changes,
        verification,
        questions: store
            .snapshot()
            .questions
            .iter()
            .filter(|q| q.revision == revision.id)
            .map(|q| {
                (ctx.shown)(&format!(
                    "{}: {}\n{}",
                    q.id,
                    q.question,
                    q.answer
                        .as_ref()
                        .map(|a| format!("Answer: {a}"))
                        .unwrap_or_else(|| "Awaiting your answer".into())
                ))
            })
            .collect(),
        status: describe(ctx.run_dir)?,
        approved,
        can_implement: execution.is_none() && !unanswered,
        can_resume: execution.is_some_and(|e| {
            matches!(e.status, declass_agent::plans::ExecutionStatus::Paused)
                && e.turns < e.max_turns
        }) && !unanswered
            && approved,
        steps: execution
            .map(|e| {
                e.steps
                    .iter()
                    .map(|s| PlanStepProgress {
                        id: s.id.clone(),
                        status: match s.status {
                            declass_agent::plans::StepStatus::Pending => "Pending",
                            declass_agent::plans::StepStatus::InProgress => "In progress",
                            declass_agent::plans::StepStatus::DoneUnverified => {
                                "Completed — unverified"
                            }
                            declass_agent::plans::StepStatus::Verified => "Checks passed",
                            declass_agent::plans::StepStatus::Blocked => "Blocked",
                        }
                        .into(),
                        note: s.note.as_ref().map(|text| (ctx.shown)(text)),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }))
}

pub(super) fn publish(ctx: &TurnCtx<'_>) -> Result<()> {
    if let Some(workspace) = ctx.io.screen.workspace() {
        workspace.plan(snapshot(ctx)?);
    }
    Ok(())
}

fn projection_warning(ctx: &TurnCtx<'_>, store: &Store) {
    if let Some(error) = store.projection_error() {
        say(
            ctx,
            &format!("The plan was saved, but its Markdown view could not be refreshed: {error}"),
        );
    }
}

fn audit_action(ctx: &TurnCtx<'_>, action: &str, identity: &PlanIdentity) {
    ctx.audit.record(declass_boundary::audit::AuditEvent::Plan {
        action: action.into(),
        revision: identity.revision.clone(),
        digest: identity.digest.clone(),
        step_id: None,
        check_id: None,
    });
}

pub(super) fn review(ctx: &TurnCtx<'_>, revision: Option<&str>, edit: bool) -> Result<()> {
    let store = load(ctx.run_dir)?;
    let current = store
        .current()
        .context("There is no saved plan yet. Use /plan <task>.")?;
    if let Some(id) = revision
        && id != current.id
    {
        say(
            ctx,
            &store.render_markdown_for(id).map_err(anyhow::Error::msg)?,
        );
        return Ok(());
    }
    publish(ctx)?;
    match ctx.io.screen.workspace() {
        Some(workspace) => workspace.open_plan(edit),
        None => say(ctx, &store.render_markdown()),
    }
    Ok(())
}

pub(super) fn pause(ctx: &TurnCtx<'_>, reason: &str) -> Result<()> {
    let mut store = load(ctx.run_dir)?;
    let before = store.snapshot().execution.as_ref().map(|e| e.status);
    if before.is_some() {
        store.pause(reason).map_err(anyhow::Error::msg)?;
    }
    if store.snapshot().execution.as_ref().map(|e| e.status) != before
        && let Some(revision) = store.current()
    {
        audit_action(
            ctx,
            "pause",
            &PlanIdentity {
                revision: revision.id.clone(),
                digest: revision.digest.clone(),
            },
        );
    }
    Ok(())
}

pub(super) fn pause_goal(goals: &mut crate::goals::Store, reason: &str) -> Result<()> {
    if goals.current().is_some_and(|g| {
        matches!(
            g.state,
            crate::goals::State::Active | crate::goals::State::Waiting
        )
    }) {
        goals.pause(reason)?;
    }
    Ok(())
}

fn enter_planning(
    session: &mut Session<'_>,
    ctx: &TurnCtx<'_>,
    goals: &mut crate::goals::Store,
) -> Result<bool> {
    let changed = !session.is_planning();
    pause(
        ctx,
        "Planning pauses implementation. Use /plan resume rN when ready.",
    )?;
    pause_goal(
        goals,
        "Planning mode pauses automatic goals. Use /goal resume after leaving planning.",
    )?;
    session.set_planning(true).map_err(anyhow::Error::msg)?;
    Ok(changed)
}

fn result(changed: bool) -> Effect {
    if changed {
        Effect::Restart
    } else {
        Effect::Done
    }
}

fn request_prompt(message: String, changed: bool, ctx: &TurnCtx<'_>) -> Effect {
    if changed {
        ctx.io.inbox.requeue(vec![if message.starts_with('/') {
            format!("/{message}")
        } else {
            message
        }]);
        Effect::Restart
    } else {
        Effect::Prompt(message)
    }
}

pub(super) async fn text_action(
    raw: &str,
    session: &mut Session<'_>,
    ctx: &TurnCtx<'_>,
    goals: &mut crate::goals::Store,
) -> Result<Effect> {
    match plan_action(raw) {
        PlanAction::Invalid(error) => bail!(error),
        PlanAction::Status => {
            say(
                ctx,
                &format!(
                    "{}\n{}",
                    super::plan_status(session.is_planning()),
                    describe(ctx.run_dir)?
                ),
            );
            Ok(Effect::Done)
        }
        PlanAction::Review(revision) => {
            review(ctx, revision, false)?;
            Ok(Effect::Done)
        }
        PlanAction::On => {
            let changed = enter_planning(session, ctx, goals)?;
            say(
                ctx,
                if changed {
                    "Switching to planning; stopping runtime services."
                } else {
                    super::plan_status(true)
                },
            );
            Ok(result(changed))
        }
        PlanAction::Off => {
            pause(ctx, "Implementation stays paused after /plan off.")?;
            let changed = session.is_planning();
            session.set_planning(false).map_err(anyhow::Error::msg)?;
            say(ctx, super::plan_status(false));
            Ok(result(changed))
        }
        PlanAction::Task(task) => {
            let changed = enter_planning(session, ctx, goals)?;
            Ok(request_prompt(task.to_owned(), changed, ctx))
        }
        PlanAction::Revise(feedback) => {
            let store = load(ctx.run_dir)?;
            let id = store
                .current()
                .context("There is no saved plan to revise.")?
                .id
                .clone();
            let changed = enter_planning(session, ctx, goals)?;
            Ok(request_prompt(
                format!(
                    "Revise saved plan {id} using this feedback. Read the saved plan and propose a new revision; do not implement it.\n\n{feedback}"
                ),
                changed,
                ctx,
            ))
        }
        PlanAction::Edit => {
            let changed = enter_planning(session, ctx, goals)?;
            if changed {
                ctx.io.inbox.requeue(vec!["/plan edit".into()]);
                return Ok(Effect::Restart);
            }
            if ctx.io.screen.workspace().is_some() {
                review(ctx, None, true)?;
            } else if ctx.io.tty {
                edit_plain(ctx).await?;
            } else {
                say(
                    ctx,
                    "The plan editor needs an interactive terminal. Use /plan review and /plan revise <feedback>.",
                );
            }
            Ok(result(changed))
        }
        PlanAction::Pause => {
            pause(ctx, "Paused by you. Use /plan resume rN to continue.")?;
            say(ctx, &describe(ctx.run_dir)?);
            Ok(Effect::Done)
        }
        PlanAction::Approve(id) | PlanAction::Implement(id) | PlanAction::Resume(id) => {
            let store = load(ctx.run_dir)?;
            let identity = checked_identity(&store, id)?;
            let action = match plan_action(raw) {
                PlanAction::Approve(_) => UiAction::Approve { identity },
                PlanAction::Implement(_) => UiAction::Implement { identity },
                _ => UiAction::Resume { identity },
            };
            ui_action(action, session, ctx, goals).await
        }
    }
}

pub(super) async fn ui_action(
    action: UiAction,
    session: &mut Session<'_>,
    ctx: &TurnCtx<'_>,
    goals: &mut crate::goals::Store,
) -> Result<Effect> {
    match action {
        UiAction::Edit { identity } => {
            validate_identity(&load(ctx.run_dir)?, &identity)?;
            let changed = enter_planning(session, ctx, goals)?;
            if changed {
                ctx.io
                    .inbox
                    .requeue_plan_action(UiAction::Edit { identity });
                return Ok(Effect::Restart);
            }
            review(ctx, None, true)?;
            Ok(Effect::Done)
        }
        UiAction::Answer {
            question_id,
            identity,
            choice_id,
            text,
        } => {
            answer(
                session,
                ctx,
                goals,
                &question_id,
                identity.as_ref(),
                choice_id.as_deref(),
                &text,
            )
            .await
        }
        UiAction::Pause { identity } => {
            validate_identity(&load(ctx.run_dir)?, &identity)?;
            pause(ctx, "Paused by you. Use /plan resume rN to continue.")?;
            Ok(Effect::Done)
        }
        UiAction::Revise { identity, feedback } => {
            validate_identity(&load(ctx.run_dir)?, &identity)?;
            ensure!(
                !feedback.trim().is_empty(),
                "Describe the revision you want."
            );
            let changed = enter_planning(session, ctx, goals)?;
            Ok(request_prompt(
                format!(
                    "Revise saved plan {} using this feedback. Read the saved plan and propose a new revision; do not implement it.\n\n{feedback}",
                    identity.revision
                ),
                changed,
                ctx,
            ))
        }
        UiAction::Save { identity, draft } => {
            validate_identity(&load(ctx.run_dir)?, &identity)?;
            let changed = enter_planning(session, ctx, goals)?;
            let mut store = load(ctx.run_dir)?;
            let revision = store
                .revise(Some(&identity.revision), draft)
                .map_err(anyhow::Error::msg)?;
            projection_warning(ctx, &store);
            audit_action(
                ctx,
                "revise",
                &PlanIdentity {
                    revision: revision.id.clone(),
                    digest: revision.digest.clone(),
                },
            );
            say(
                ctx,
                &format!(
                    "Saved {}. Review it before approving or implementing.",
                    revision.id
                ),
            );
            if let Some(workspace) = ctx.io.screen.workspace() {
                workspace.plan_saved(snapshot(ctx)?.context("Saved plan is unavailable")?);
            }
            Ok(result(changed))
        }
        UiAction::Approve { identity } => {
            validate_identity(&load(ctx.run_dir)?, &identity)?;
            let changed = enter_planning(session, ctx, goals)?;
            let mut store = load(ctx.run_dir)?;
            store
                .approve(&identity.revision, &identity.digest)
                .map_err(anyhow::Error::msg)?;
            projection_warning(ctx, &store);
            audit_action(ctx, "approve", &identity);
            say(
                ctx,
                &format!("{} approved. No implementation started.", identity.revision),
            );
            publish(ctx)?;
            Ok(result(changed))
        }
        action @ (UiAction::Implement { .. } | UiAction::Resume { .. }) => {
            let (identity, resume) = match action {
                UiAction::Implement { identity } => (identity, false),
                UiAction::Resume { identity } => (identity, true),
                _ => unreachable!(),
            };
            let mut store = load(ctx.run_dir)?;
            validate_identity(&store, &identity)?;
            pause_goal(goals, "Plan execution is active; this goal remains paused.")?;
            if resume {
                store.resume(&identity.revision, &identity.digest)
            } else {
                store.implement(&identity.revision, &identity.digest, ctx.plan_turns)
            }
            .map_err(anyhow::Error::msg)?;
            projection_warning(ctx, &store);
            if !resume {
                audit_action(ctx, "approve", &identity);
            }
            audit_action(ctx, if resume { "resume" } else { "implement" }, &identity);
            let changed = session.is_planning();
            if let Err(error) = session.set_planning(false) {
                store
                    .pause("Execution could not restore its runtime; resume when ready.")
                    .map_err(anyhow::Error::msg)?;
                bail!(error);
            }
            say(
                ctx,
                &format!(
                    "Implementing {} within the remaining session budgets.",
                    identity.revision
                ),
            );
            publish(ctx)?;
            Ok(result(changed))
        }
    }
}

pub(super) fn active(run_dir: &Path) -> Result<bool> {
    Ok(load(run_dir)?.active())
}

pub(super) fn prompt(run_dir: &Path) -> Result<Option<String>> {
    Ok(load(run_dir)?.prompt())
}

pub(super) fn begin_turn(run_dir: &Path) -> Result<()> {
    load(run_dir)?.begin_turn().map_err(anyhow::Error::msg)
}

pub(super) fn finish_turn(ctx: &TurnCtx<'_>, end: &TurnEnd) -> Result<()> {
    let mut store = load(ctx.run_dir)?;
    store.finish_turn(end).map_err(anyhow::Error::msg)?;
    if let (Some(revision), Some(execution)) =
        (store.current(), store.snapshot().execution.as_ref())
    {
        let action = match execution.status {
            declass_agent::plans::ExecutionStatus::Active => "continue",
            declass_agent::plans::ExecutionStatus::Waiting => "waiting",
            declass_agent::plans::ExecutionStatus::Paused => "pause",
            declass_agent::plans::ExecutionStatus::Completed => "complete",
            declass_agent::plans::ExecutionStatus::Exhausted => "exhausted",
            declass_agent::plans::ExecutionStatus::Cancelled => "cancel",
            declass_agent::plans::ExecutionStatus::Superseded => "supersede",
        };
        audit_action(
            ctx,
            action,
            &PlanIdentity {
                revision: revision.id.clone(),
                digest: revision.digest.clone(),
            },
        );
    }
    Ok(())
}

#[derive(Clone)]
struct PendingQuestion {
    text: String,
    options: declass_agent::questions::QuestionOptions,
    identity: Option<PlanIdentity>,
}

fn pending(session: &Session<'_>, run_dir: &Path) -> Result<Option<PendingQuestion>> {
    let Some(TurnEnd::Asked {
        question,
        options: Some(options),
    }) = session.history().last().and_then(|turn| turn.end.as_ref())
    else {
        return Ok(None);
    };
    let store = load(run_dir)?;
    let recorded = store
        .snapshot()
        .questions
        .iter()
        .find(|q| q.id == options.id);
    if recorded.is_some_and(|q| q.answer.is_some()) {
        return Ok(None);
    }
    let identity = match recorded {
        Some(q) => {
            let Some(revision) = store.current().filter(|r| r.id == q.revision) else {
                return Ok(None);
            };
            Some(PlanIdentity {
                revision: revision.id.clone(),
                digest: revision.digest.clone(),
            })
        }
        None => None,
    };
    Ok(Some(PendingQuestion {
        text: question.clone(),
        options: options.clone(),
        identity,
    }))
}

pub(super) fn publish_question(session: &Session<'_>, ctx: &TurnCtx<'_>) -> Result<()> {
    let question = pending(session, ctx.run_dir)?;
    for line in ctx
        .io
        .inbox
        .discard_stale_question(question.as_ref().map(|q| q.options.id.as_str()))
    {
        say(
            ctx,
            &format!(
                "Earlier queued text was not sent because its question changed. Send it again if still needed:\n{line}"
            ),
        );
    }
    if let Some(question) = question.as_ref() {
        ctx.io.inbox.question_mark(&question.options.id);
    }
    if let Some(workspace) = ctx.io.screen.workspace() {
        workspace.plan_question(question.map(|q| declass_tui::workspace::PlanQuestion {
            id: q.options.id.clone(),
            identity: q.identity,
            question: q.text,
            options: q.options,
        }));
    }
    Ok(())
}

pub(super) fn has_question(session: &Session<'_>, run_dir: &Path) -> Result<bool> {
    Ok(pending(session, run_dir)?.is_some())
}

pub(super) async fn answer_text(
    raw: &str,
    session: &mut Session<'_>,
    ctx: &TurnCtx<'_>,
    goals: &mut crate::goals::Store,
) -> Result<Effect> {
    let (id, value) = raw
        .trim()
        .split_once(char::is_whitespace)
        .context("Use /answer QUESTION_ID OPTION_NUMBER or /answer QUESTION_ID --text <answer>.")?;
    let question =
        pending(session, ctx.run_dir)?.context("There is no pending structured question.")?;
    let value = value.trim();
    if let Some(text) = value.strip_prefix("--text ") {
        answer(
            session,
            ctx,
            goals,
            id,
            question.identity.as_ref(),
            None,
            text,
        )
        .await
    } else {
        let index: usize = value
            .parse()
            .context("Select a numbered option, or use --text for a custom answer.")?;
        let choice = question
            .options
            .choices
            .get(index.checked_sub(1).context("Options start at 1.")?)
            .context("There is no such question option.")?;
        answer(
            session,
            ctx,
            goals,
            id,
            question.identity.as_ref(),
            Some(&choice.id),
            "",
        )
        .await
    }
}

pub(super) async fn answer_freeform(
    text: &str,
    session: &mut Session<'_>,
    ctx: &TurnCtx<'_>,
    goals: &mut crate::goals::Store,
) -> Result<Effect> {
    let question = pending(session, ctx.run_dir)?.context("There is no pending question.")?;
    answer(
        session,
        ctx,
        goals,
        &question.options.id,
        question.identity.as_ref(),
        None,
        text,
    )
    .await
}

async fn answer(
    session: &mut Session<'_>,
    ctx: &TurnCtx<'_>,
    goals: &mut crate::goals::Store,
    id: &str,
    identity: Option<&PlanIdentity>,
    choice: Option<&str>,
    text: &str,
) -> Result<Effect> {
    let question = pending(session, ctx.run_dir)?.context("This question is no longer pending.")?;
    ensure!(
        question.options.id == id,
        "This answer refers to an older question."
    );
    ensure!(
        question.identity.as_ref() == identity,
        "The plan changed since this question was shown."
    );
    let answer = match choice {
        Some(id) => question
            .options
            .choices
            .iter()
            .find(|c| c.id == id)
            .context("This option does not belong to the question.")?
            .label
            .clone(),
        None => {
            ensure!(
                question.options.allow_freeform,
                "Choose one of this question's listed options."
            );
            ensure!(
                !text.trim().is_empty() && text.len() <= 64 * 1024,
                "Provide a nonempty answer of at most 64 KiB."
            );
            text.trim().to_owned()
        }
    };
    let mut store = load(ctx.run_dir)?;
    if let Some(identity) = identity {
        validate_identity(&store, identity)?;
        store
            .answer_question(&identity.revision, id, &answer)
            .map_err(anyhow::Error::msg)?;
        audit_action(ctx, "answer", identity);
        if !session.is_planning()
            && store
                .snapshot()
                .execution
                .as_ref()
                .is_some_and(|e| e.status == declass_agent::plans::ExecutionStatus::Waiting)
        {
            store
                .resume(&identity.revision, &identity.digest)
                .map_err(anyhow::Error::msg)?;
        }
    }
    if !session.is_planning()
        && goals
            .current()
            .is_some_and(|g| g.state == crate::goals::State::Waiting)
    {
        goals.resume()?;
    }
    ctx.io.inbox.release_question_input();
    if let Some(workspace) = ctx.io.screen.workspace() {
        workspace.plan_question(None);
    }
    Ok(Effect::Prompt(format!("Answer to question {id}: {answer}")))
}

async fn editor_line(ctx: &TurnCtx<'_>, prompt: &str) -> Result<Option<String>> {
    ctx.io.screen.text(prompt);
    loop {
        if ctx.io.leave.load(Ordering::SeqCst) {
            return Ok(None);
        }
        if let Some(line) = ctx.io.inbox.pop() {
            if line == "/cancel" {
                return Ok(None);
            }
            return Ok(Some(line));
        }
        if ctx.io.inbox.drained() {
            return Ok(None);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn edit_plain(ctx: &TurnCtx<'_>) -> Result<()> {
    let store = load(ctx.run_dir)?;
    let revision = store.current().context("There is no saved plan to edit.")?;
    let identity = PlanIdentity {
        revision: revision.id.clone(),
        digest: revision.digest.clone(),
    };
    let mut draft = local_draft(&revision.draft, ctx.shown.as_ref());
    say(
        ctx,
        "Plan editor: title, objective, scope, non-goals, assumptions, decisions; step N title|details|paths|acceptance|checks; add; remove N; move N M; save; cancel.\nUse a trailing backslash for multiline values. Lists use one entry per line. /cancel cancels the editor.",
    );
    loop {
        let Some(command) = editor_line(ctx, "plan edit> ").await? else {
            break;
        };
        let words: Vec<&str> = command.split_whitespace().collect();
        match words.as_slice() {
            ["cancel"] => break,
            ["save"] => {
                let mut store = load(ctx.run_dir)?;
                validate_identity(&store, &identity)?;
                match store.revise(Some(&identity.revision), draft.clone()) {
                    Ok(revision) => {
                        projection_warning(ctx, &store);
                        audit_action(
                            ctx,
                            "revise",
                            &PlanIdentity {
                                revision: revision.id.clone(),
                                digest: revision.digest.clone(),
                            },
                        );
                        say(
                            ctx,
                            &format!("Saved {}. Review before implementing.", revision.id),
                        );
                        break;
                    }
                    Err(error) => {
                        say(ctx, &error);
                        continue;
                    }
                }
            }
            ["add"] => draft.steps.push(declass_agent::plans::Step {
                title: "New step".into(),
                description: "Describe the change".into(),
                ..Default::default()
            }),
            ["remove", index] => {
                if let Ok(index) = index.parse::<usize>()
                    && index > 0
                    && index <= draft.steps.len()
                {
                    draft.steps.remove(index - 1);
                } else {
                    say(ctx, "Choose an existing step number.");
                }
            }
            ["move", from, to] => {
                if let (Ok(from), Ok(to)) = (from.parse::<usize>(), to.parse::<usize>())
                    && from > 0
                    && to > 0
                    && from <= draft.steps.len()
                    && to <= draft.steps.len()
                {
                    let step = draft.steps.remove(from - 1);
                    draft.steps.insert(to - 1, step);
                } else {
                    say(ctx, "Choose existing source and destination step numbers.");
                }
            }
            [
                field @ ("title" | "objective" | "scope" | "non-goals" | "assumptions"
                | "decisions"),
            ] => {
                let Some(value) = editor_line(ctx, "new value> ").await? else {
                    break;
                };
                let lines = || {
                    value
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(str::to_owned)
                        .collect()
                };
                match *field {
                    "title" => draft.title = value,
                    "objective" => draft.objective = value,
                    "scope" => draft.scope = lines(),
                    "non-goals" => draft.non_goals = lines(),
                    "assumptions" => draft.assumptions = lines(),
                    _ => draft.decisions = lines(),
                }
            }
            [
                "step",
                index,
                field @ ("title" | "details" | "paths" | "acceptance" | "checks"),
            ] => {
                let index = index.parse::<usize>().ok().and_then(|n| n.checked_sub(1));
                let Some(step) = index.and_then(|i| draft.steps.get_mut(i)) else {
                    say(ctx, "Choose an existing step number.");
                    continue;
                };
                let Some(value) = editor_line(ctx, "new value> ").await? else {
                    break;
                };
                let lines = || {
                    value
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                };
                match *field {
                    "title" => step.title = value,
                    "details" => step.description = value,
                    "paths" => step.paths = lines(),
                    "acceptance" => step.acceptance = lines(),
                    _ => {
                        step.checks = lines()
                            .into_iter()
                            .map(|command| {
                                step.checks
                                    .iter()
                                    .find(|c| c.command == command)
                                    .cloned()
                                    .unwrap_or(declass_agent::plans::Check {
                                        command,
                                        description: "Automated check".into(),
                                        ..Default::default()
                                    })
                            })
                            .collect()
                    }
                }
            }
            _ => say(
                ctx,
                "Unknown editor action. Use a section name, step N FIELD, add, remove N, move N M, save, or cancel.",
            ),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_require_an_exact_revision_and_never_fall_back_to_task_text() {
        for text in [
            "implement",
            "implement latest",
            "implement r1 extra",
            "approve r0",
            "resume 1",
            "off please",
        ] {
            assert!(
                matches!(plan_action(text), PlanAction::Invalid(_)),
                "{text}"
            );
        }
        assert_eq!(plan_action("implement r12"), PlanAction::Implement("r12"));
        assert_eq!(
            plan_action("task review the billing code"),
            PlanAction::Task("review the billing code")
        );
        assert_eq!(
            plan_action("Review the billing code"),
            PlanAction::Task("Review the billing code")
        );
    }
}
