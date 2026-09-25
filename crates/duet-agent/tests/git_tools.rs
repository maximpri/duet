// SPDX-License-Identifier: GPL-3.0-or-later
//! The git tools against temporary repositories with history: a key that was
//! committed and later removed, a sensitive file's history, protected source,
//! an author email; both presenter modes; the commit rules; and the run's
//! diff, rollback and resume after a commit.

use duet_agent::git_tools::GitTools;
use duet_agent::journal::WriteJournal;
use duet_agent::tools::{Ctx, Outcome, dispatch};
use duet_boundary::audit::{AuditHandle, AuditLog, Line};
use duet_boundary::engine::Engine;
use duet_boundary::policy::Policy;
use duet_boundary::view::{PassThrough, Presenter};
use duet_git::{Git, Identity};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

const KEY: &str = "sk_live_Qm8vT2xW9pL4nR7kZ3cY6bH1";
const EMAIL: &str = "amelia.velanwick42@mailbox-311.net";
const OLD_BALANCE: &str = "8977066";
const BALANCE: &str = "5530917";
const SAUCE: &str = "4242424";
const PRICING_BODY: &str = "917331";

struct Repo {
    _dir: tempfile::TempDir,
    ws: PathBuf,
    run: PathBuf,
    git: Git,
}

fn sh(repo: &Repo, args: &[&str], env: &[(&str, &str)]) -> String {
    let out = repo.git.run(&repo.ws, args, env, None).unwrap();
    String::from_utf8_lossy(&out).trim().to_owned()
}

fn write(repo: &Repo, path: &str, text: &str) {
    let p = repo.ws.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// Two commits by a person with an email: the first holds a key, sensitive
/// data and protected source; the second removes the key and changes the data.
fn repo() -> Repo {
    let d = tempfile::tempdir().unwrap();
    let ws = d.path().canonicalize().unwrap().join("ws");
    let run = ws.join(".duet/runs/r1");
    std::fs::create_dir_all(&run).unwrap();
    let git = Git::locate().unwrap();
    let r = Repo {
        _dir: d,
        ws,
        run,
        git,
    };
    sh(&r, &["init", "-q", "-b", "main"], &[]);
    r.git.exclude_state_dir(&r.ws).unwrap();
    let who = [
        ("GIT_AUTHOR_NAME", "Amelia Velanwick"),
        ("GIT_AUTHOR_EMAIL", EMAIL),
    ];
    write(&r, "README.md", "Payments service.\n");
    write(
        &r,
        "src/pay.rs",
        &format!("pub const KEY: &str = \"{KEY}\";\npub fn pay() {{}}\n"),
    );
    write(
        &r,
        "data/orders.csv",
        &format!("id,total\n1,{OLD_BALANCE}\n"),
    );
    write(
        &r,
        "src/sauce.rs",
        &format!("pub fn sauce() -> u32 {{ {SAUCE} }}\n"),
    );
    write(
        &r,
        "src/pricing.rs",
        &format!("/// Price.\npub fn price() -> u32 {{\n    {PRICING_BODY}\n}}\n"),
    );
    sh(&r, &["add", "-A"], &[]);
    sh(&r, &["commit", "-q", "-m", "Initial import"], &who);
    write(
        &r,
        "src/pay.rs",
        "pub fn key() -> String {\n    std::env::var(\"PAY_KEY\").unwrap()\n}\npub fn pay() {}\n",
    );
    write(&r, "data/orders.csv", &format!("id,total\n1,{BALANCE}\n"));
    write(
        &r,
        "src/sauce.rs",
        &format!("pub fn sauce() -> u32 {{ {SAUCE} + 1 }}\n"),
    );
    sh(&r, &["add", "-A"], &[]);
    sh(
        &r,
        &["commit", "-q", "-m", "Read the key from the environment"],
        &who,
    );
    r
}

fn policy() -> Policy {
    Policy {
        sensitive_globs: vec!["data/**".into(), ".env*".into()],
        command_output_sensitive: true,
        detect_secrets: true,
        detect_pii: true,
        detect_entropy: true,
        bulky_tokens: 4000,
        bulky_file_tokens: 4000,
        interface_only: vec!["src/pricing.rs".into()],
        sealed: vec!["src/sauce.rs".into()],
        ..Policy::default()
    }
}

fn hybrid(r: &Repo) -> std::sync::Arc<Engine> {
    let engine = Engine::open(&r.run, policy(), None).unwrap();
    let files = r.git.list_files(&r.ws).unwrap();
    engine.prime(&r.ws, &files, "Refactor payments.");
    engine
}

fn operator() -> Identity {
    Identity::parse("Olive Operator <olive@example.test>").unwrap()
}

fn ctx<'a>(
    r: &'a Repo,
    presenter: &'a dyn Presenter,
    journal: &'a mut WriteJournal,
    tools: &'a GitTools,
    audit: Option<&'a AuditHandle>,
) -> Ctx<'a> {
    Ctx {
        workspace: &r.ws,
        run_dir: &r.run,
        sandbox: duet_sandbox::detect().unwrap_or(duet_sandbox::SandboxKind::Seatbelt),
        git: &r.git,
        presenter,
        journal,
        command_timeout: Duration::from_secs(30),
        network: false,
        checks: &[],
        audit,
        interrupted: None,
        web: None,
        git_tools: Some(tools),
        lsp: None,
    }
}

async fn call(ctx: &mut Ctx<'_>, name: &str, args: Value) -> String {
    let Value::Object(args) = args else {
        unreachable!()
    };
    match dispatch(ctx, name, &args).await {
        Outcome::Result(s) => s,
        Outcome::Error(e) => format!("error: {e}"),
        other => format!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hybrid_history_never_shows_sensitive_values_or_protected_source() {
    let r = repo();
    let engine = hybrid(&r);
    let mut journal = WriteJournal::open(&r.run).unwrap();
    let tools = GitTools {
        commit: true,
        author: Some(operator()),
        user_files: vec![],
    };
    let mut c = ctx(&r, engine.as_ref(), &mut journal, &tools, None);
    let mut seen = Vec::new();
    for (name, args) in [
        ("git_status", json!({})),
        ("git_log", json!({})),
        ("git_log", json!({"path": "data/orders.csv"})),
        ("git_show", json!({"rev": "HEAD~1"})),
        ("git_show", json!({"rev": "HEAD"})),
        ("git_show", json!({"rev": "HEAD~1", "path": "src/pay.rs"})),
        (
            "git_show",
            json!({"rev": "HEAD~1", "path": "data/orders.csv"}),
        ),
        (
            "git_show",
            json!({"rev": "HEAD~1", "path": "src/pricing.rs"}),
        ),
        ("git_show", json!({"rev": "HEAD~1", "path": "src/sauce.rs"})),
        ("git_blame", json!({"path": "src/pay.rs"})),
        ("git_blame", json!({"path": "data/orders.csv"})),
        ("git_blame", json!({"path": "src/sauce.rs"})),
        ("git_blame", json!({"path": "src/pricing.rs"})),
        ("git_log", json!({"path": "src/sauce.rs"})),
    ] {
        let out = call(&mut c, name, args.clone()).await;
        seen.push((format!("{name} {args}"), out));
    }
    for (what, out) in &seen {
        for secret in [KEY, EMAIL, OLD_BALANCE, BALANCE, SAUCE, PRICING_BODY] {
            assert!(!out.contains(secret), "{what} showed {secret}:\n{out}");
        }
        assert!(
            !out.contains("sauce.rs") || out.starts_with("error"),
            "{what}:\n{out}"
        );
    }
    let by = |prefix: &str| {
        seen.iter()
            .find(|(w, _)| w.starts_with(prefix))
            .map(|(_, o)| o.as_str())
            .unwrap()
    };
    let log = by("git_log {}");
    assert!(
        log.contains("Read the key from the environment") && log.contains("Amelia Velanwick"),
        "{log}"
    );
    assert!(log.contains("⟨"), "the email is a placeholder: {log}");
    // The first commit: the removed key is scanned out of the public file's
    // diff, the data file is held locally, protected source is withheld.
    let first = by(r#"git_show {"rev":"HEAD~1"}"#);
    assert!(first.contains("pub fn pay()"), "{first}");
    // Git's own headers are not mistaken for data.
    assert!(first.contains("new file mode 100644"), "{first}");
    assert!(first.contains("ask_local"), "{first}");
    assert!(
        first.contains("src/pricing.rs is interface-only"),
        "{first}"
    );
    let second = by(r#"git_show {"rev":"HEAD"}"#);
    assert!(
        second.contains("-pub const KEY") && second.contains("PAY_KEY"),
        "{second}"
    );
    let blame = by(r#"git_blame {"path":"src/pay.rs"}"#);
    assert!(
        blame.contains("Amelia Velanwick") && blame.contains("pub fn pay()"),
        "{blame}"
    );
    assert!(
        by(r#"git_show {"rev":"HEAD~1","path":"src/sauce.rs"}"#).contains("not available"),
        "sealed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn passthrough_history_is_shown_as_it_is() {
    let r = repo();
    let p = PassThrough { max_bytes: 60_000 };
    let mut journal = WriteJournal::open(&r.run).unwrap();
    let tools = GitTools {
        commit: false,
        author: None,
        user_files: vec![],
    };
    let mut c = ctx(&r, &p, &mut journal, &tools, None);
    let log = call(&mut c, "git_log", json!({"max_count": 1})).await;
    assert!(
        log.contains(EMAIL) && log.contains("latest 1 commits"),
        "{log}"
    );
    let show = call(&mut c, "git_show", json!({"rev": "HEAD"})).await;
    assert!(
        show.contains(&format!("-pub const KEY: &str = \"{KEY}\";")),
        "{show}"
    );
    let old = call(
        &mut c,
        "git_show",
        json!({"rev": "HEAD~1", "path": "data/orders.csv"}),
    )
    .await;
    assert!(old.contains(OLD_BALANCE), "{old}");
    let blame = call(
        &mut c,
        "git_blame",
        json!({"path": "src/pay.rs", "start_line": 2, "end_line": 2}),
    )
    .await;
    assert!(
        blame.contains("lines 2-2 of 4") && blame.contains("PAY_KEY"),
        "{blame}"
    );
    let status = call(&mut c, "git_status", json!({})).await;
    assert!(
        status.contains("branch: main") && status.contains("no changes"),
        "{status}"
    );
    // Not offered, not run.
    let commit = call(&mut c, "git_commit", json!({"message": "x"})).await;
    assert!(commit.contains("not available"), "{commit}");
    for bad in [json!({"rev": "--output=/tmp/x"}), json!({"rev": "nope"})] {
        let e = call(&mut c, "git_show", bad).await;
        assert!(e.starts_with("error:"), "{e}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn commits_follow_the_rules_and_leave_diff_rollback_and_resume_intact() {
    let r = repo();
    let engine = hybrid(&r);
    let base = r.git.head(&r.ws).unwrap().unwrap();
    let audit_path = r.run.join("audit.jsonl");
    let audit = AuditHandle::new(AuditLog::open(&audit_path).unwrap());
    let tools = GitTools {
        commit: true,
        author: Some(operator()),
        user_files: vec![],
    };
    let mut journal = WriteJournal::open(&r.run).unwrap();
    {
        let mut c = ctx(&r, engine.as_ref(), &mut journal, &tools, Some(&audit));
        for (path, content) in [
            ("src/new.rs", "pub fn fresh() {}\n"),
            ("data/report.csv", "id,total\n1,3\n"),
            ("notes.log", "debug\n"),
        ] {
            let out = call(
                &mut c,
                "write_file",
                json!({"path": path, "content": content}),
            )
            .await;
            assert!(out.starts_with("created"), "{out}");
        }
        std::fs::write(r.ws.join(".gitignore"), "*.log\n").unwrap();
        let edit = call(
            &mut c,
            "edit_file",
            json!({"path": "src/pay.rs", "edits": [{"old": "pub fn pay() {}", "new": "pub fn pay() { todo!() }"}]}),
        )
        .await;
        assert!(edit.starts_with("edited"), "{edit}");

        // Refusals: a file the run did not write, a sensitive one, placeholders,
        // known sensitive values and personal data in the message.
        for (args, expect) in [
            (
                json!({"message": "m", "paths": ["README.md"]}),
                "not written in this run",
            ),
            (
                json!({"message": "m", "paths": ["data/report.csv"]}),
                "sensitive",
            ),
            (
                json!({"message": "m", "paths": ["notes.log"]}),
                "git ignores it",
            ),
            (json!({"message": "Use ⟨secret:PAY_KEY#1⟩"}), "placeholder"),
            (
                json!({"message": format!("Totals now {BALANCE}")}),
                "sensitive value",
            ),
            (
                json!({"message": format!("Thanks {EMAIL}")}),
                "sensitive value",
            ),
            (json!({"message": "  "}), "empty"),
        ] {
            let out = call(&mut c, "git_commit", args.clone()).await;
            assert!(out.contains(expect), "{args}: {out}");
        }
        assert_eq!(r.git.head(&r.ws).unwrap().as_deref(), Some(base.as_str()));

        let out = call(
            &mut c,
            "git_commit",
            json!({"message": "Add fresh and stub pay"}),
        )
        .await;
        assert!(
            out.starts_with("committed ")
                && out.contains("on main (2 files): src/new.rs, src/pay.rs"),
            "{out}"
        );
        assert!(
            out.contains("not committed:")
                && out.contains("data/report.csv (it is sensitive")
                && out.contains("notes.log (git ignores it)"),
            "{out}"
        );
        // The run's diff still shows everything it changed since it started.
        let diff = call(&mut c, "diff", json!({})).await;
        assert!(diff.contains("todo!()") && diff.contains("fresh"), "{diff}");
    }
    let head = r.git.head(&r.ws).unwrap().unwrap();
    assert_ne!(head, base);
    let meta = sh(
        &r,
        &["log", "-1", "--format=%an <%ae>|%cn <%ce>|%s|%P"],
        &[],
    );
    assert_eq!(
        meta,
        format!(
            "Olive Operator <olive@example.test>|Olive Operator <olive@example.test>|Add fresh and stub pay|{base}"
        )
    );
    let status = sh(&r, &["status", "--porcelain"], &[]);
    assert!(
        !status.contains("src/pay.rs") && !status.contains("src/new.rs"),
        "{status}"
    );

    // The audit log records the commit (hash and paths), never the message.
    let log = std::fs::read_to_string(&audit_path).unwrap();
    assert!(!log.contains("stub pay"), "{log}");
    let commits: Vec<_> = duet_boundary::audit::read(&audit_path)
        .unwrap()
        .into_iter()
        .filter_map(|l| match l {
            Line::Event(e) => match e.event {
                duet_boundary::audit::AuditEvent::GitCommit { hash, paths } => Some((hash, paths)),
                _ => None,
            },
            Line::Request(_) => None,
        })
        .collect();
    assert_eq!(
        commits,
        [(
            head.clone(),
            vec!["src/new.rs".to_owned(), "src/pay.rs".to_owned()]
        )]
    );

    // Rollback: a write interrupted after the commit is rolled back on resume.
    std::fs::write(
        r.run.join("writes/99.before"),
        std::fs::read(r.ws.join("src/pay.rs")).unwrap(),
    )
    .unwrap();
    duet_fs::private::append_line(
        &r.run.join("writes.jsonl"),
        r#"{"state":"pending","n":99,"path":"src/pay.rs","existed":true}"#,
    )
    .unwrap();
    std::fs::write(r.ws.join("src/pay.rs"), "half-written").unwrap();
    let restored = WriteJournal::recover(&r.run, &r.ws).unwrap();
    assert_eq!(restored, vec![PathBuf::from("src/pay.rs")]);
    assert!(!sh(&r, &["status", "--porcelain"], &[]).contains("src/pay.rs"));

    // Resume: a new journal (a new process) still knows what the run wrote,
    // and the diff still starts at the run's base.
    let mut resumed = WriteJournal::open(&r.run).unwrap();
    let mut c = ctx(&r, engine.as_ref(), &mut resumed, &tools, Some(&audit));
    let edit = call(
        &mut c,
        "write_file",
        json!({"path": "src/new.rs", "content": "pub fn fresh() -> u8 { 1 }\n"}),
    )
    .await;
    assert!(edit.starts_with("replaced"), "{edit}");
    let out = call(
        &mut c,
        "git_commit",
        json!({"message": "Return one", "paths": ["src/new.rs"]}),
    )
    .await;
    assert!(out.contains("(1 file): src/new.rs"), "{out}");
    let again = call(
        &mut c,
        "git_commit",
        json!({"message": "Again", "paths": ["src/pay.rs"]}),
    )
    .await;
    assert!(again.contains("nothing to commit"), "{again}");
    let diff = call(&mut c, "diff", json!({})).await;
    assert!(diff.contains("todo!()") && diff.contains("-> u8"), "{diff}");
    assert_eq!(
        std::fs::read_to_string(r.run.join("git-base")).unwrap(),
        base,
        "the base is recorded once"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn commits_need_an_operator_identity() {
    let r = repo();
    let p = PassThrough { max_bytes: 60_000 };
    let mut journal = WriteJournal::open(&r.run).unwrap();
    // No git.author and no owner git configuration.
    let tools = GitTools {
        commit: true,
        author: None,
        user_files: vec![],
    };
    let mut c = ctx(&r, &p, &mut journal, &tools, None);
    call(
        &mut c,
        "write_file",
        json!({"path": "a.rs", "content": "x"}),
    )
    .await;
    let out = call(&mut c, "git_commit", json!({"message": "m"})).await;
    assert!(out.contains("no operator identity"), "{out}");
    sh(&r, &["config", "user.name", "Repo Owner"], &[]);
    sh(&r, &["config", "user.email", "owner@example.test"], &[]);
    let out = call(&mut c, "git_commit", json!({"message": "m"})).await;
    assert!(out.starts_with("committed"), "{out}");
    assert_eq!(sh(&r, &["log", "-1", "--format=%an"], &[]), "Repo Owner");
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test(flavor = "multi_thread")]
async fn git_through_a_command_points_at_the_git_tools() {
    let r = repo();
    let engine = hybrid(&r);
    let mut journal = WriteJournal::open(&r.run).unwrap();
    let tools = GitTools {
        commit: false,
        author: None,
        user_files: vec![],
    };
    let mut c = ctx(&r, engine.as_ref(), &mut journal, &tools, None);
    let out = call(&mut c, "run_command", json!({"command": "git log -p"})).await;
    assert!(out.contains("use the git_status, git_log"), "{out}");
    assert!(!out.contains(KEY), "{out}");
}
