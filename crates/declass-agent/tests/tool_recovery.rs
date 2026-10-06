// SPDX-License-Identifier: GPL-3.0-or-later
//! Recovery messages through tool dispatch, including the write journal and
//! the boundary that holds sensitive command output locally.
#![cfg(any(target_os = "macos", target_os = "linux"))]

use declass_agent::egress::Network;
use declass_agent::journal::WriteJournal;
use declass_agent::tools::{Ctx, Outcome, dispatch};
use declass_boundary::audit::{AuditHandle, AuditLog};
use declass_boundary::engine::Engine;
use declass_boundary::policy::Policy;
use declass_boundary::view::{PassThrough, Presenter};
use declass_git::Git;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

struct Fixture {
    _dir: tempfile::TempDir,
    workspace: PathBuf,
    run: PathBuf,
    git: Git,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().canonicalize().unwrap().join("workspace");
        let run = workspace.join(".declass/runs/recovery");
        std::fs::create_dir_all(&run).unwrap();
        Self {
            _dir: dir,
            workspace,
            run,
            git: Git::locate().unwrap(),
        }
    }

    fn context<'a>(
        &'a self,
        presenter: &'a dyn Presenter,
        journal: &'a mut WriteJournal,
        audit: Option<&'a AuditHandle>,
    ) -> Ctx<'a> {
        Ctx {
            workspace: &self.workspace,
            run_dir: &self.run,
            sandbox: declass_sandbox::detect().unwrap_or(declass_sandbox::SandboxKind::Seatbelt),
            git: &self.git,
            presenter,
            journal,
            command_timeout: Duration::from_secs(10),
            network: &Network::Off,
            checks: &[],
            audit,
            interrupted: None,
            web: None,
            git_tools: None,
            lsp: None,
        }
    }
}

async fn call(ctx: &mut Ctx<'_>, name: &str, args: Value) -> Outcome {
    let Value::Object(args) = args else {
        unreachable!()
    };
    dispatch(ctx, name, &args).await
}

fn result(outcome: Outcome) -> String {
    match outcome {
        Outcome::Result(text) => text,
        other => panic!("expected tool result, got {other:?}"),
    }
}

fn error(outcome: Outcome) -> String {
    match outcome {
        Outcome::Error(text) => text,
        other => panic!("expected tool error, got {other:?}"),
    }
}

#[tokio::test]
async fn failed_edit_batches_leave_both_files_and_the_journal_untouched() {
    let fixture = Fixture::new();
    let original = "header\nfunction target() {\n    original();\n}\n";
    std::fs::write(fixture.workspace.join("one.txt"), original).unwrap();
    std::fs::write(fixture.workspace.join("two.txt"), "second file\n").unwrap();
    let presenter = PassThrough { max_bytes: 4096 };
    let mut journal = WriteJournal::open(&fixture.run).unwrap();
    let first_record = journal.next_record();
    let mut ctx = fixture.context(&presenter, &mut journal, None);

    let missing_top_level = error(
        call(
            &mut ctx,
            "edit_file",
            json!({"edits": [{"path": "one.txt", "old": "header", "new": "changed"}]}),
        )
        .await,
    );
    assert!(missing_top_level.contains("top-level string `path`"));

    let conflicting_targets = error(
        call(
            &mut ctx,
            "edit_file",
            json!({"path": "one.txt", "edits": [
                {"old": "header", "new": "changed"},
                {"path": "two.txt", "old": "second file", "new": "corrupted"}
            ]}),
        )
        .await,
    );
    assert!(conflicting_targets.contains("not inside `edits`"));

    let staged_failure = error(
        call(
            &mut ctx,
            "edit_file",
            json!({"path": "one.txt", "edits": [
                {"old": "header", "new": "header\ninserted\nlines"},
                {"old": "function target() {\n    stale();", "new": "changed"}
            ]}),
        )
        .await,
    );
    assert!(
        staged_failure.contains("no edits applied"),
        "{staged_failure}"
    );
    assert!(
        staged_failure.contains("appears at line 2"),
        "{staged_failure}"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("one.txt")).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("two.txt")).unwrap(),
        "second file\n"
    );
    assert_eq!(ctx.journal.next_record(), first_record);
    assert!(ctx.journal.paths().is_empty());
    assert!(!fixture.run.join("writes.jsonl").exists());

    let valid = result(
        call(
            &mut ctx,
            "edit_file",
            json!({"path": "one.txt", "edits": [{"old": "header", "new": "updated"}]}),
        )
        .await,
    );
    assert!(valid.contains("edited one.txt"), "{valid}");
    assert!(
        std::fs::read_to_string(fixture.workspace.join("one.txt"))
            .unwrap()
            .starts_with("updated\n")
    );
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("two.txt")).unwrap(),
        "second file\n"
    );
    assert_eq!(ctx.journal.paths(), vec![PathBuf::from("one.txt")]);
    assert_eq!(ctx.journal.next_record(), first_record + 1);
}

#[tokio::test]
async fn sensitive_command_failure_does_not_expose_recovery_clues() {
    if cfg!(target_os = "linux") && std::env::var_os("DECLASS_SANDBOX_BRIDGE").is_none() {
        eprintln!("skipped: DECLASS_SANDBOX_BRIDGE is not set");
        return;
    }
    let fixture = Fixture::new();
    let secret = "private-command-value-938271";
    std::fs::create_dir_all(fixture.workspace.join("data")).unwrap();
    std::fs::write(fixture.workspace.join("data/private.txt"), secret).unwrap();
    let policy = Policy {
        sensitive_globs: vec!["data/**".into()],
        ..Policy::default()
    };
    let presenter = Engine::open(&fixture.run, policy, None).unwrap();
    presenter.prime(&fixture.workspace, &["data/private.txt".into()], "");
    let audit_path = fixture.run.join("audit.jsonl");
    let audit = AuditHandle::new(AuditLog::open(&audit_path).unwrap());
    let mut journal = WriteJournal::open(&fixture.run).unwrap();
    let mut ctx = fixture.context(presenter.as_ref(), &mut journal, Some(&audit));

    let ordinary = result(
        call(
            &mut ctx,
            "run_command",
            json!({"command": "printf 'connection %s\\n' refused >&2; exit 7"}),
        )
        .await,
    );
    assert!(ordinary.contains("[command guidance]"), "{ordinary}");
    assert!(ordinary.contains("networking is disabled"), "{ordinary}");

    let held = result(
        call(
            &mut ctx,
            "run_command",
            json!({"command": "cat data/private.txt >&2; printf 'connection %s\\n' refused >&2; exit 7", "sensitive_data": true}),
        )
        .await,
    );
    assert!(held.contains("ask_local"), "{held}");
    for hidden in [
        secret,
        "connection refused",
        "[command guidance]",
        "networking is disabled",
    ] {
        assert!(!held.contains(hidden), "exposed {hidden}: {held}");
    }
    let handle = held.split_whitespace().next().unwrap();
    let raw = std::fs::read_to_string(fixture.run.join("handles").join(handle)).unwrap();
    assert!(
        raw.contains(secret),
        "sensitive command did not read the private file"
    );
    assert!(
        raw.contains("connection refused"),
        "sensitive command did not emit the failure"
    );
    let audit_text = std::fs::read_to_string(audit_path).unwrap();
    assert!(!audit_text.contains(secret), "{audit_text}");
}
