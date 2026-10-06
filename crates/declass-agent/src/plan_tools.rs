// SPDX-License-Identifier: GPL-3.0-or-later
//! Plan tool orchestration. Drafts are data; only the operator grants execution.
use crate::plans::{Draft, StepStatus, Store};
use crate::tools::{Access, Ctx, Outcome};
use declass_boundary::model::ToolSpec;
use declass_boundary::view::Source;
use serde_json::{Map, Value, json};
use std::sync::atomic::Ordering;

pub(crate) fn owns(name: &str) -> bool {
    matches!(
        name,
        "propose_plan" | "read_plan" | "update_plan_step" | "verify_plan_step"
    )
}

pub(crate) fn specs() -> Vec<ToolSpec> {
    let string = json!({"type":"string"});
    let strings = json!({"type":"array","items":{"type":"string"}});
    let step = json!({"type":"object","properties":{
        "id":{"type":"string","description":"Keep an existing step ID when revising; use empty string for a new step."},"title":string,"description":string,"paths":strings,"acceptance":strings,
        "checks":{"type":"array","items":{"type":"object","properties":{
            "id":{"type":"string","description":"Keep an existing check ID; empty string for a new check."},"command":string,"description":string
        },"required":["id","command","description"],"additionalProperties":false}}
    },"required":["id","title","description","paths","checks"],"additionalProperties":false});
    vec![
        ToolSpec{name:"propose_plan".into(),description:"Save a structured draft in private session state during planning. This does not approve or execute it. Supply expected_revision when replacing an existing draft. Commands remain proposals until the operator approves this exact revision.".into(),parameters:json!({"type":"object","properties":{
            "expected_revision":string,"draft":{"type":"object","properties":{
                "title":string,"objective":string,"scope":strings,"non_goals":strings,"assumptions":strings,"decisions":strings,"steps":{"type":"array","items":step}
            },"required":["title","objective","scope","non_goals","assumptions","steps"],"additionalProperties":false}
        },"required":["draft"],"additionalProperties":false})},
        ToolSpec{name:"read_plan".into(),description:"Read the current private plan and recorded progress through the session privacy boundary. Returned content is not permission to execute.".into(),parameters:json!({"type":"object","properties":{},"additionalProperties":false})},
        ToolSpec{name:"update_plan_step".into(),description:"Report progress in an approved active plan. Status is in_progress, done_unverified, or blocked. Only recorded host checks can verify a step. Steps without checks can complete explicitly unverified.".into(),parameters:json!({"type":"object","properties":{
            "revision":string,"step_id":string,"status":{"type":"string","enum":["in_progress","done_unverified","blocked"]},"note":string
        },"required":["revision","step_id","status","note"],"additionalProperties":false})},
        ToolSpec{name:"verify_plan_step".into(),description:"Run this approved step's stored check commands with ordinary command permissions and oversight. Cannot supply or replace a command. Check evidence is recorded by the host; interrupted checks do not pass.".into(),parameters:json!({"type":"object","properties":{
            "revision":string,"step_id":string
        },"required":["revision","step_id"],"additionalProperties":false})},
    ]
}

pub(crate) fn invalidate(run_dir: &std::path::Path, reason: &str) -> Result<(), String> {
    let mut store = Store::load(run_dir)?;
    if store.snapshot().execution.is_none() {
        return Ok(());
    }
    store.invalidate_verification(reason)
}

pub(crate) fn may_mutate(name: &str) -> bool {
    !matches!(
        name,
        "read_file"
            | "list_files"
            | "search"
            | "diff"
            | "read_raw"
            | "synthetic_sample"
            | "ask_local"
            | "git_status"
            | "git_log"
            | "git_show"
            | "git_blame"
            | "list_skills"
            | "load_skill"
            | "reply"
            | "ask_operator"
            | "read_plan"
            | "propose_plan"
            | "update_plan_step"
            | "verify_plan_step"
            | "finish"
    )
}

/// Retain the exact bounded command output privately before committing host evidence.
pub(crate) fn save_output(run_dir: &std::path::Path, output: &[u8]) -> Result<String, String> {
    let absolute = if run_dir.is_absolute() {
        run_dir.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(run_dir)
    };
    let relative = format!("plan-checks/{}.txt", uuid::Uuid::new_v4());
    let path = absolute.join(&relative);
    let rel = path
        .strip_prefix("/")
        .map_err(|_| "invalid run directory")?;
    let pinned = declass_fs::pinned::PinnedParent::open(std::path::Path::new("/"), rel, true)
        .map_err(|e| e.to_string())?;
    declass_fs::private::write_private_pinned(&pinned, output).map_err(|e| e.to_string())?;
    Ok(relative)
}

fn stopped(ctx: &Ctx<'_>) -> bool {
    ctx.interrupted.is_some_and(|i| i.load(Ordering::SeqCst))
}

pub(crate) async fn call(
    ctx: &mut Ctx<'_>,
    oversight: &crate::oversight::Oversight,
    planning: bool,
    name: &str,
    args: &Map<String, Value>,
    deadline: Option<tokio::time::Instant>,
) -> Outcome {
    match call_inner(ctx, oversight, planning, name, args, deadline).await {
        Ok(text) => Outcome::Result(text),
        Err(error) => Outcome::Error(error),
    }
}

async fn call_inner(
    ctx: &mut Ctx<'_>,
    oversight: &crate::oversight::Oversight,
    planning: bool,
    name: &str,
    args: &Map<String, Value>,
    deadline: Option<tokio::time::Instant>,
) -> Result<String, String> {
    let mut store = Store::load(ctx.run_dir)?;
    match name {
        "read_plan" => {
            let raw =
                serde_json::to_string(store.snapshot()).map_err(|_| "cannot serialize plan")?;
            Ok(ctx.presenter.sanitize_message(&raw))
        }
        "propose_plan" => {
            if !planning {
                return Err("enter planning before proposing or revising a plan".into());
            }
            let draft: Draft =
                serde_json::from_value(args.get("draft").cloned().ok_or("missing draft")?)
                    .map_err(|_| "invalid structured plan")?;
            let expected = args
                .get("expected_revision")
                .map(|v| v.as_str().ok_or("expected_revision must be a string"))
                .transpose()?;
            let saved = store.revise(expected, draft)?;
            ctx.record(declass_boundary::audit::AuditEvent::Plan {
                action: "proposed".into(),
                revision: saved.id.clone(),
                digest: saved.digest.clone(),
                step_id: None,
                check_id: None,
            });
            Ok(format!(
                "Draft {} saved. Await operator approval; no execution was authorized.",
                saved.id
            ))
        }
        "update_plan_step" => {
            if planning {
                return Err(crate::planning::REFUSAL.into());
            }
            let revision = crate::tools::string_arg(args, "revision")?;
            let step = crate::tools::string_arg(args, "step_id")?;
            let status = match crate::tools::string_arg(args, "status")? {
                "in_progress" => StepStatus::InProgress,
                "done_unverified" => StepStatus::DoneUnverified,
                "blocked" => StepStatus::Blocked,
                _ => return Err("model progress cannot claim host verification".into()),
            };
            store.report_step(
                revision,
                step,
                status,
                Some(crate::tools::string_arg(args, "note")?),
            )?;
            if let Some(plan) = store.current() {
                ctx.record(declass_boundary::audit::AuditEvent::Plan {
                    action: "step_progress".into(),
                    revision: plan.id.clone(),
                    digest: plan.digest.clone(),
                    step_id: Some(step.into()),
                    check_id: None,
                });
            }
            Ok("Progress recorded. Verification labels come only from host evidence.".into())
        }
        "verify_plan_step" => {
            if planning {
                return Err(crate::planning::REFUSAL.into());
            }
            if !store.active() {
                return Err("no approved plan execution is active".into());
            }
            store.invalidate_verification("verification commands may modify the workspace")?;
            verify_step(
                ctx,
                oversight,
                &mut store,
                crate::tools::string_arg(args, "revision")?,
                crate::tools::string_arg(args, "step_id")?,
                deadline,
            )
            .await
        }
        _ => Err("unknown plan tool".into()),
    }
}

async fn verify_step(
    ctx: &Ctx<'_>,
    oversight: &crate::oversight::Oversight,
    store: &mut Store,
    revision: &str,
    step_id: &str,
    deadline: Option<tokio::time::Instant>,
) -> Result<String, String> {
    let plan = store.snapshot().current().ok_or("no plan")?;
    if plan.id != revision {
        return Err("plan revision is stale".into());
    }
    let step = plan
        .draft
        .steps
        .iter()
        .find(|s| s.id == step_id)
        .ok_or("unknown plan step")?
        .clone();
    if step.checks.is_empty() {
        return Err(
            "this step has no automated checks; report it done_unverified when complete".into(),
        );
    }
    let mut shown = Vec::new();
    for check in &step.checks {
        if stopped(ctx) || deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
            return Err("verification interrupted or session time exhausted".into());
        }
        let ticket = store.begin_verification(revision, step_id, &check.id)?;
        let args = json!({"command":ticket.command,"sensitive_data":false});
        let approval = crate::oversight::review(
            oversight,
            "run_command",
            args.as_object().expect("object"),
            ctx.presenter,
            ctx.audit,
        );
        let result = match approval {
            Ok(()) if !stopped(ctx) && deadline.is_none_or(|d| tokio::time::Instant::now() < d) => {
                let timeout = deadline.map_or(ctx.command_timeout, |d| {
                    ctx.command_timeout
                        .min(d.saturating_duration_since(tokio::time::Instant::now()))
                });
                crate::tools::sandboxed(ctx, &ticket.command, timeout, Access::Ordinary).await
            }
            Ok(()) => Err("verification interrupted or session time exhausted".into()),
            Err(e) => Err(e),
        };
        let (exit, timed_out, output) = match result {
            Ok(o) => (o.exit_code, o.timed_out, crate::tools::render_output(&o)),
            Err(e) => (
                None,
                false,
                format!("could not run check: {e}").into_bytes(),
            ),
        };
        let interrupted = stopped(ctx);
        let output_path = save_output(ctx.run_dir, &output)?;
        store.finish_verification_with_output(
            &ticket,
            exit,
            timed_out,
            interrupted,
            Some(&declass_fs::sha256_hex(&output)),
            Some(&output_path),
        )?;
        if let Some(plan) = store.current() {
            ctx.record(declass_boundary::audit::AuditEvent::Plan {
                action: if exit == Some(0) && !timed_out && !interrupted {
                    "check_passed"
                } else {
                    "check_failed"
                }
                .into(),
                revision: plan.id.clone(),
                digest: plan.digest.clone(),
                step_id: Some(step_id.into()),
                check_id: Some(check.id.clone()),
            });
        }
        shown.push(ctx.presenter.present(
            &Source::Command {
                command: ticket.command.clone(),
                exit_code: exit,
            },
            &output,
        ));
        if interrupted {
            return Err("verification interrupted; evidence is not a pass".into());
        }
    }
    Ok(shown.join("\n"))
}

/// Proposed checks never inherit the more privileged owner acceptance-check access.
pub(crate) async fn before_finish(
    ctx: &Ctx<'_>,
    oversight: &crate::oversight::Oversight,
    deadline: Option<tokio::time::Instant>,
) -> Result<(), String> {
    let mut store = Store::load(ctx.run_dir)?;
    let Some(execution) = store.snapshot().execution.as_ref() else {
        return Ok(());
    };
    use crate::plans::ExecutionStatus;
    if matches!(
        execution.status,
        ExecutionStatus::Completed | ExecutionStatus::Cancelled | ExecutionStatus::Superseded
    ) {
        return Ok(());
    }
    if !store.active() {
        return Ok(());
    }
    let revision = execution.revision.clone();
    if !execution
        .steps
        .iter()
        .all(|s| matches!(s.status, StepStatus::DoneUnverified | StepStatus::Verified))
    {
        return Err(
            "plan steps are not complete; report every implementation step before finishing".into(),
        );
    }
    let steps = store
        .snapshot()
        .current()
        .ok_or("approved plan missing")?
        .draft
        .steps
        .clone();
    store.invalidate_verification("final verification reruns all approved checks")?;
    let mut reports = Vec::new();
    for step in steps {
        if !step.checks.is_empty() {
            reports.push(
                verify_step(ctx, oversight, &mut store, &revision, &step.id, deadline).await?,
            );
            store.report_step(
                &revision,
                &step.id,
                StepStatus::DoneUnverified,
                Some("final checks observed"),
            )?;
        }
    }
    if !store.ready_to_finish() {
        return Err(format!(
            "required plan checks failed; repair and verify before finishing:\n{}",
            reports.join("\n")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::WriteJournal;
    use crate::oversight::{ApproveMode, Oversight};
    use crate::plans::{Check, Step, VerificationStatus};
    use declass_boundary::{engine::Engine, policy::Policy};
    use std::path::PathBuf;
    use std::time::Duration;

    struct Fixture {
        _dir: tempfile::TempDir,
        ws: PathBuf,
        run: PathBuf,
        git: declass_git::Git,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let ws = dir.path().canonicalize().unwrap();
            let run = ws.join(".declass/runs/plan-test");
            std::fs::create_dir_all(&run).unwrap();
            Self {
                _dir: dir,
                ws,
                run,
                git: declass_git::Git::locate().unwrap(),
            }
        }
        fn plan(&self, commands: &[&str], active: bool) -> (Store, String, String) {
            let mut store = Store::load(&self.run).unwrap();
            let revision = store
                .revise(
                    None,
                    Draft {
                        title: "Repair".into(),
                        objective: "Repair and check".into(),
                        steps: vec![Step {
                            title: "Repair".into(),
                            description: "Make the change".into(),
                            checks: commands
                                .iter()
                                .map(|c| Check {
                                    command: (*c).into(),
                                    description: "Check result".into(),
                                    ..Check::default()
                                })
                                .collect(),
                            ..Step::default()
                        }],
                        ..Draft::default()
                    },
                )
                .unwrap();
            let step = revision.draft.steps[0].id.clone();
            if active {
                store.approve(&revision.id, &revision.digest).unwrap();
                store.implement(&revision.id, &revision.digest, 10).unwrap();
                store.begin_turn().unwrap();
                store
                    .report_step(&revision.id, &step, StepStatus::DoneUnverified, None)
                    .unwrap();
            }
            (store, revision.id, step)
        }
        fn ctx<'a>(
            &'a self,
            journal: &'a mut WriteJournal,
            presenter: &'a dyn declass_boundary::view::Presenter,
        ) -> Ctx<'a> {
            Ctx {
                workspace: &self.ws,
                run_dir: &self.run,
                sandbox: declass_sandbox::detect().unwrap(),
                git: &self.git,
                presenter,
                journal,
                command_timeout: Duration::from_secs(5),
                network: &crate::egress::Network::Off,
                checks: &[],
                audit: None,
                interrupted: None,
                web: None,
                git_tools: None,
                lsp: None,
            }
        }
    }
    #[test]
    fn private_evidence_cannot_be_redirected_through_a_link() {
        let f = Fixture::new();
        let outside = f.ws.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, f.run.join("plan-checks")).unwrap();
        assert!(save_output(&f.run, b"private check output").is_err());
        assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
    }
    #[tokio::test]
    async fn drafting_and_paused_plans_cannot_execute_stored_commands() {
        let f = Fixture::new();
        let (_, revision, step) = f.plan(&["printf changed > bypass"], false);
        let p = declass_boundary::view::PassThrough { max_bytes: 4096 };
        let mut journal = WriteJournal::open(&f.run).unwrap();
        let mut ctx = f.ctx(&mut journal, &p);
        let args = json!({"revision":revision,"step_id":step});
        for planning in [true, false] {
            assert!(matches!(
                call(
                    &mut ctx,
                    &Oversight::default(),
                    planning,
                    "verify_plan_step",
                    args.as_object().unwrap(),
                    None,
                )
                .await,
                Outcome::Error(_)
            ));
        }
        assert!(!f.ws.join("bypass").exists());
    }
    #[tokio::test]
    async fn approved_checks_still_obey_command_oversight_and_cannot_be_replaced() {
        let f = Fixture::new();
        let (_, revision, step) = f.plan(&["printf approved > approved"], true);
        let p = declass_boundary::view::PassThrough { max_bytes: 4096 };
        let mut journal = WriteJournal::open(&f.run).unwrap();
        let mut ctx = f.ctx(&mut journal, &p);
        let args =
            json!({"revision":revision,"step_id":step,"command":"printf injected > injected"});
        let oversight = Oversight {
            mode: ApproveMode::All,
            ..Oversight::default()
        };
        let _ = call(
            &mut ctx,
            &oversight,
            false,
            "verify_plan_step",
            args.as_object().unwrap(),
            None,
        )
        .await;
        assert!(!f.ws.join("approved").exists());
        assert!(!f.ws.join("injected").exists());
        let state = Store::load(&f.run).unwrap();
        assert_eq!(
            state.snapshot().execution.as_ref().unwrap().evidence[0].status,
            VerificationStatus::Failed
        );
        let _ = call(
            &mut ctx,
            &Oversight::default(),
            false,
            "verify_plan_step",
            args.as_object().unwrap(),
            None,
        )
        .await;
        assert!(f.ws.join("approved").exists());
        assert!(!f.ws.join("injected").exists());
    }
    #[tokio::test]
    async fn final_checks_rerun_and_failed_checks_require_repair() {
        let f = Fixture::new();
        let (_, _, _) = f.plan(&["test -f accepted"], true);
        let p = declass_boundary::view::PassThrough { max_bytes: 4096 };
        let mut journal = WriteJournal::open(&f.run).unwrap();
        let ctx = f.ctx(&mut journal, &p);
        assert!(
            before_finish(&ctx, &Oversight::default(), None)
                .await
                .is_err()
        );
        std::fs::write(f.ws.join("accepted"), "fixed").unwrap();
        assert!(
            before_finish(&ctx, &Oversight::default(), None)
                .await
                .is_ok()
        );
        std::fs::remove_file(f.ws.join("accepted")).unwrap();
        assert!(
            before_finish(&ctx, &Oversight::default(), None)
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn plan_reads_cross_the_existing_privacy_boundary() {
        let f = Fixture::new();
        let secret = "sk-proj-PlanPrivateCanary_123456789012345678901234567890";
        let (mut store, _, _) = f.plan(&["printf harmless"], false);
        let mut draft = store.current().unwrap().draft.clone();
        draft.objective = format!("Use credential {secret}");
        let previous = store.current().unwrap().id.clone();
        store.revise(Some(&previous), draft).unwrap();
        let engine = Engine::open(
            &f.run,
            Policy {
                detect_secrets: true,
                ..Policy::default()
            },
            None,
        )
        .unwrap();
        engine.prime(&f.ws, &[], secret);
        let mut journal = WriteJournal::open(&f.run).unwrap();
        let mut ctx = f.ctx(&mut journal, engine.as_ref());
        let out = call(
            &mut ctx,
            &Oversight::default(),
            true,
            "read_plan",
            &Map::new(),
            None,
        )
        .await;
        let Outcome::Result(text) = out else {
            panic!("read failed")
        };
        assert!(
            !text.contains(secret),
            "private plan content bypassed sanitization"
        );
        assert!(
            store.render_markdown().contains(secret),
            "local artifact is retained privately"
        );
    }
    #[tokio::test]
    async fn proposed_checks_cannot_read_owner_protected_source() {
        let f = Fixture::new();
        std::fs::write(f.ws.join("protected.rs"), "private protected source").unwrap();
        let (_, revision, step) = f.plan(&["cat protected.rs"], true);
        let engine = Engine::open(
            &f.run,
            Policy {
                interface_only: vec!["protected.rs".into()],
                ..Policy::default()
            },
            None,
        )
        .unwrap();
        let mut journal = WriteJournal::open(&f.run).unwrap();
        let mut ctx = f.ctx(&mut journal, engine.as_ref());
        let args = json!({"revision":revision,"step_id":step});
        let out = call(
            &mut ctx,
            &Oversight::default(),
            false,
            "verify_plan_step",
            args.as_object().unwrap(),
            None,
        )
        .await;
        assert!(!format!("{out:?}").contains("private protected source"));
        let store = Store::load(&f.run).unwrap();
        assert_eq!(
            store
                .snapshot()
                .execution
                .as_ref()
                .unwrap()
                .evidence
                .last()
                .unwrap()
                .status,
            VerificationStatus::Failed
        );
        // The existing owner-configured acceptance path does have protected-source access.
        let (failed, _) = crate::tools::run_checks(&ctx, &["test -r protected.rs".into()]).await;
        assert!(!failed);
    }
    #[tokio::test]
    async fn verification_deadline_stops_the_batch_and_retains_private_evidence() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        let (_, revision, step) = f.plan(
            &["printf started; sleep 5", "printf late > deadline-bypass"],
            true,
        );
        let p = declass_boundary::view::PassThrough { max_bytes: 4096 };
        let mut journal = WriteJournal::open(&f.run).unwrap();
        let mut ctx = f.ctx(&mut journal, &p);
        let args = json!({"revision":revision,"step_id":step});
        let result = call(
            &mut ctx,
            &Oversight::default(),
            false,
            "verify_plan_step",
            args.as_object().unwrap(),
            Some(tokio::time::Instant::now() + Duration::from_millis(150)),
        )
        .await;
        assert!(matches!(result, Outcome::Error(_)));
        assert!(!f.ws.join("deadline-bypass").exists());
        let store = Store::load(&f.run).unwrap();
        let evidence = &store.snapshot().execution.as_ref().unwrap().evidence;
        assert_eq!(evidence.len(), 1);
        assert_ne!(evidence[0].exit_code, Some(0));
        assert_eq!(evidence[0].status, VerificationStatus::Failed);
        assert!(evidence[0].timestamp_ms.is_some());
        let path = f.run.join(evidence[0].output_path.as_ref().unwrap());
        let output = std::fs::read(&path).unwrap();
        assert!(
            evidence[0].timed_out
                || String::from_utf8_lossy(&output).contains(
                    "could not run check: verification interrupted or session time exhausted"
                ),
            "the command must time out or be refused before execution"
        );
        assert_eq!(
            evidence[0].output_digest.as_ref().unwrap(),
            &declass_fs::sha256_hex(&output)
        );
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[tokio::test]
    async fn unchecked_completion_is_explicit_and_does_not_require_manual_acceptance() {
        let f = Fixture::new();
        let (_, _, _) = f.plan(&[], true);
        let p = declass_boundary::view::PassThrough { max_bytes: 4096 };
        let mut journal = WriteJournal::open(&f.run).unwrap();
        let ctx = f.ctx(&mut journal, &p);
        before_finish(&ctx, &Oversight::default(), None)
            .await
            .unwrap();
        let store = Store::load(&f.run).unwrap();
        let e = store.snapshot().execution.as_ref().unwrap();
        assert_eq!(e.steps[0].status, StepStatus::DoneUnverified);
        assert!(e.evidence.is_empty());
    }
}
