// SPDX-License-Identifier: GPL-3.0-or-later
//! `edit_protected`: changes to source the frontier may not see.
//!
//! The frontier describes the change; the presenter has it implemented where
//! the source may be read (the local model); the host writes the result with
//! the guarded write path and runs the verifying command as a check (protected
//! source readable, sensitive data not). Only pass/fail, the check output as
//! the presenter shows it, and an interface-level summary go back. A failed
//! check gives the local model its raw output and one more attempt.

use crate::tools::{Ctx, MAX_READ_BYTES, fs_err, run_checks, string_arg};
use declass_boundary::audit::AuditEvent;
use declass_boundary::view::{ImplementRequest, Source};
use declass_fs::Precondition;
use serde_json::{Map, Value};

/// Local implementation attempts per call.
pub const ATTEMPTS: usize = 2;
/// Characters of failing check output the local model sees on a retry.
const FEEDBACK_CHARS: usize = 12_000;

/// The last `FEEDBACK_CHARS` characters of a report (the end holds the failures).
fn feedback_tail(report: &[u8]) -> String {
    let text = String::from_utf8_lossy(report);
    let n = text.chars().count();
    text.chars()
        .skip(n.saturating_sub(FEEDBACK_CHARS))
        .collect()
}

pub async fn edit_protected(
    ctx: &mut Ctx<'_>,
    args: &Map<String, Value>,
) -> Result<String, String> {
    let raw = string_arg(args, "path")?;
    let rel = declass_fs::writable_relative(raw).map_err(fs_err)?;
    let spec = string_arg(args, "spec")?;
    let tests = match args.get("tests") {
        Some(Value::Object(t)) => Some((string_arg(t, "path")?, string_arg(t, "content")?)),
        Some(Value::Null) | None => None,
        Some(_) => return Err("`tests` must be an object {path, content}".into()),
    };
    let commands: Vec<String> = match args.get("command").and_then(Value::as_str) {
        Some(c) if !c.trim().is_empty() => vec![c.to_owned()],
        _ => ctx.checks.to_vec(),
    };
    if commands.is_empty() {
        return Err(
            "no checks are configured; give `command` (e.g. the test run that verifies the change)"
                .into(),
        );
    }
    let mut tests_written = false;
    let mut feedback: Option<String> = None;
    let mut last_error = String::new();
    for attempt in 1..=ATTEMPTS {
        let bytes = declass_fs::read_file(ctx.workspace, &rel, MAX_READ_BYTES).map_err(fs_err)?;
        let current =
            String::from_utf8(bytes.clone()).map_err(|_| "file is not valid UTF-8".to_owned())?;
        let request = ImplementRequest {
            spec,
            tests: tests.map(|(_, content)| content),
            feedback: feedback.as_deref(),
        };
        let done = match ctx.presenter.implement_protected(&rel, &current, &request) {
            None => {
                return Err(format!(
                    "{raw} is not protected; change it with edit_file or write_file"
                ));
            }
            Some(Err(e)) => {
                // The model's output was unusable: tell it why and try again.
                last_error = e.clone();
                feedback = Some(format!("your previous answer was not used: {e}"));
                continue;
            }
            Some(Ok(done)) => done,
        };
        if let (Some((path, content)), false) = (tests, tests_written) {
            let test_rel = declass_fs::writable_relative(path).map_err(fs_err)?;
            let content = ctx.presenter.resolve_for_write(&test_rel, content)?;
            ctx.journal
                .write(
                    ctx.workspace,
                    &test_rel,
                    content.as_bytes(),
                    &Precondition::Any,
                )
                .map_err(fs_err)?;
            tests_written = true;
        }
        let pre = Precondition::Sha256(declass_fs::sha256_hex(&bytes));
        ctx.journal
            .write(ctx.workspace, &rel, done.content.as_bytes(), &pre)
            .map_err(fs_err)?;
        let (failed, report) = run_checks(ctx, &commands).await;
        let shown = ctx.presenter.present(&Source::Checks, &report);
        let verdict = if failed { "FAILED" } else { "passed" };
        ctx.record(AuditEvent::ProtectedEdit {
            path: rel.display().to_string(),
            attempt: attempt as u32,
            checks_passed: !failed,
        });
        if !failed || attempt == ATTEMPTS {
            return Ok(format!(
                "{raw}: the local model's change was written (attempt {attempt} of {ATTEMPTS}); {}.\n\
                 checks {verdict}{}.\n{shown}",
                done.summary,
                if failed {
                    "; the change is kept: refine the spec and call edit_protected again"
                } else {
                    ""
                },
            ));
        }
        feedback = Some(feedback_tail(&report));
    }
    Err(format!(
        "the local model could not produce a usable change to {raw}: {last_error}"
    ))
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use crate::journal::WriteJournal;
    use crate::tools::{Ctx, Outcome, dispatch};
    use declass_boundary::engine::Engine;
    use declass_boundary::policy::Policy;
    use declass_boundary::testing::scripted_local;
    use declass_git::Git;
    use serde_json::{Value, json};
    use std::path::Path;
    use std::time::Duration;

    const CANARY: &str = "zq5n1v8kd0rt6ym2";

    fn engine_rs(result: &str) -> String {
        format!(
            "/// Discount in basis points.\npub fn discount_bps(qty: u32) -> u32 {{\n    \
             let _rev = \"{CANARY}\";\n    let _ = qty;\n    {result}\n}}\n"
        )
    }

    async fn call(ctx: &mut Ctx<'_>, name: &str, args: Value) -> String {
        let Value::Object(args) = args else {
            unreachable!()
        };
        match dispatch(ctx, name, &args).await {
            Outcome::Result(s) | Outcome::Error(s) => s,
            other => format!("{other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn protected_edits_are_implemented_locally_and_checked() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        let run = d.path().canonicalize().unwrap().join("run");
        for (p, text) in [
            ("src/pricing/engine.rs", engine_rs("150")),
            ("src/lib.rs", "pub mod pricing;\n".into()),
        ] {
            std::fs::create_dir_all(ws.join(p).parent().unwrap()).unwrap();
            std::fs::write(ws.join(p), text).unwrap();
        }
        std::fs::create_dir_all(ws.join(".declass")).unwrap();
        std::fs::create_dir_all(&run).unwrap();
        // First attempt fails the check, the second passes.
        let (local, received) = scripted_local(vec![
            json!({"code": engine_rs("999")}).to_string(),
            json!({"code": engine_rs("1000")}).to_string(),
        ]);
        let policy = Policy {
            interface_only: vec!["src/pricing/**".into()],
            command_output_sensitive: true,
            ..Policy::default()
        };
        let engine = Engine::open(&run, policy, Some(local)).unwrap();
        let files = vec!["src/pricing/engine.rs".to_owned(), "src/lib.rs".to_owned()];
        engine.prime(&ws, &files, "");
        let git = Git::locate().unwrap();
        let mut journal = WriteJournal::open(&run).unwrap();
        let mut ctx = Ctx {
            workspace: &ws,
            run_dir: &run,
            sandbox: declass_sandbox::detect().unwrap(),
            git: &git,
            presenter: engine.as_ref(),
            journal: &mut journal,
            command_timeout: Duration::from_secs(30),
            network: &crate::egress::Network::Off,
            checks: &[],
            audit: None,
            interrupted: None,
            web: None,
            git_tools: None,
            lsp: None,
        };

        // Ordinary commands cannot read protected source; edits must go through edit_protected.
        let cat = call(
            &mut ctx,
            "run_command",
            json!({"command": "cat src/pricing/engine.rs"}),
        )
        .await;
        assert!(
            cat.contains(declass_sandbox::DENIAL_MESSAGE) && !cat.contains(CANARY),
            "{cat}"
        );
        let direct = call(
            &mut ctx,
            "write_file",
            json!({"path": "src/pricing/engine.rs", "content": "x"}),
        )
        .await;
        assert!(direct.contains("edit_protected"), "{direct}");

        // The check reads the protected file (grep) and passes only for the second version.
        let out = call(
            &mut ctx,
            "edit_protected",
            json!({
                "path": "src/pricing/engine.rs",
                "spec": "Return 1000.",
                "tests": {"path": "tests/spec.txt", "content": "discount_bps(1) == 1000"},
                "command": "grep -q '    1000$' src/pricing/engine.rs && echo 'test spec ... ok'"
            }),
        )
        .await;
        assert!(
            out.contains("attempt 2 of 2") && out.contains("checks passed"),
            "{out}"
        );
        assert!(out.contains("changed: fn discount_bps"), "{out}");
        assert!(!out.contains(CANARY), "{out}");
        assert_eq!(
            std::fs::read_to_string(ws.join("src/pricing/engine.rs")).unwrap(),
            engine_rs("1000")
        );
        assert_eq!(
            std::fs::read_to_string(ws.join("tests/spec.txt")).unwrap(),
            "discount_bps(1) == 1000"
        );
        // The retry told the local model (only) what failed.
        assert!(
            received.prompt(1).contains("<failed_attempt>"),
            "{}",
            received.prompt(1)
        );
        assert!(
            received
                .prompt(0)
                .contains("<tests>discount_bps(1) == 1000</tests>")
        );

        // Open files are not edited this way.
        let open = call(
            &mut ctx,
            "edit_protected",
            json!({"path": "src/lib.rs", "spec": "x", "command": "true"}),
        )
        .await;
        assert!(open.contains("is not protected"), "{open}");
        assert!(Path::new(&ws.join("src/lib.rs")).is_file());
    }
}
