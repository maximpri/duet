// SPDX-License-Identifier: GPL-3.0-or-later
//! Screen tests on ratatui's `TestBackend`: every screen renders, navigation,
//! the edit flows (tightening, loosening confirm and cancel, project-scope
//! refusals) and the registry round trip. No model is contacted.

use crate::app::{App, DoctorLine, Mode, Paths, Tab};
use crate::settings::{keys, screen_of};
use duet_agent::transcript::{Entry, Transcript};
use duet_boundary::audit::{self, AuditEvent, AuditLog, Line as Record, run_anchors};
use duet_boundary::model::{Item, ToolCall, Usage};
use duet_boundary::view::ViewClass;
use duet_config::{Config, Origin, REGISTRY};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use serde_json::json;
use std::path::Path;
use tempfile::TempDir;

fn doctor() -> crate::Doctor {
    Box::new(|online| {
        vec![DoctorLine {
            status: "pass".into(),
            name: "config".into(),
            detail: format!("fake check, online={online}"),
            fix: None,
        }]
    })
}

fn fixture(owner: &str, project: &str) -> (TempDir, App) {
    let d = tempfile::tempdir().unwrap();
    let ws = d.path().join("ws");
    std::fs::create_dir_all(ws.join(".duet")).unwrap();
    std::fs::create_dir_all(d.path().join("owner")).unwrap();
    let paths = Paths {
        owner: d.path().join("owner/config.toml"),
        project: ws.join(".duet/config.toml"),
        state: d.path().join("state"),
        workspace: ws,
    };
    if !owner.is_empty() {
        std::fs::write(&paths.owner, owner).unwrap();
    }
    if !project.is_empty() {
        std::fs::write(&paths.project, project).unwrap();
    }
    let app = App::new(paths, doctor()).unwrap();
    (d, app)
}

fn screen(app: &mut App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| crate::ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    buf.content
        .chunks(w as usize)
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn render(app: &mut App) -> String {
    screen(app, 160, 48)
}

fn key(app: &mut App, code: KeyCode) {
    app.key(code, KeyModifiers::NONE);
}

/// Replaces the edit buffer with `text`.
fn retype(app: &mut App, text: &str) {
    for _ in 0..80 {
        key(app, KeyCode::Backspace);
    }
    typed(app, text);
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c));
    }
}

/// Selects `k` on its settings screen.
fn select(app: &mut App, k: &str) {
    let tab = screen_of(k).unwrap();
    app.enter_tab(tab);
    app.rows[tab.index()] = keys(tab).iter().position(|s| s.key == k).unwrap();
}

fn config_audit(app: &App) -> Vec<AuditEvent> {
    audit::read(&app.paths.config_audit())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|l| match l {
            Record::Event(e) => Some(e.event),
            Record::Request(_) => None,
        })
        .collect()
}

fn reload(app: &App) -> Config {
    Config::load(&app.paths.owner, Some(&app.paths.project)).unwrap()
}

#[test]
fn every_registry_key_is_on_exactly_one_screen() {
    for s in REGISTRY {
        assert!(screen_of(s.key).is_some(), "{} has no screen", s.key);
    }
    let shown: usize = Tab::ALL.iter().map(|t| keys(*t).len()).sum();
    assert_eq!(shown, REGISTRY.len());
}

#[test]
fn settings_screens_show_every_value_and_its_origin() {
    let (_d, mut app) = fixture(
        "[frontier]\nmodel = \"owner-model\"\n",
        "[limits]\nfrontier_usd = 1.5\n",
    );
    for tab in Tab::ALL.into_iter().filter(|t| t.is_settings()) {
        app.enter_tab(tab);
        let out = screen(&mut app, 200, 60);
        for s in keys(tab) {
            assert!(out.contains(s.key), "{} missing on {tab:?}:\n{out}", s.key);
        }
    }
    select(&mut app, "frontier.model");
    let out = render(&mut app);
    assert!(out.contains("\"owner-model\""), "{out}");
    assert!(out.contains("from: owner, file"), "{out}");
    assert!(out.contains("owner-only"), "{out}");
    select(&mut app, "limits.frontier_usd");
    let out = render(&mut app);
    assert!(
        out.contains("1.5") && out.contains("from: project, file"),
        "{out}"
    );
    assert!(out.contains("a project may only lower it"), "{out}");
    select(&mut app, "data.retention_days");
    assert!(render(&mut app).contains("from: default"));
}

#[test]
fn navigation_between_screens() {
    let (_d, mut app) = fixture("", "");
    assert_eq!(app.tab(), Tab::Models);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.tab(), Tab::Sensitivity);
    key(&mut app, KeyCode::BackTab);
    key(&mut app, KeyCode::BackTab);
    assert_eq!(app.tab(), Tab::Run);
    for (c, tab) in ('1'..='7').zip(Tab::ALL) {
        key(&mut app, KeyCode::Char(c));
        assert_eq!(app.tab(), tab);
        let out = render(&mut app);
        assert!(out.contains(tab.title()), "{out}");
    }
    key(&mut app, KeyCode::Char('4'));
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Char('j'));
    assert_eq!(app.rows[Tab::Limits.index()], 2);
    key(&mut app, KeyCode::PageUp);
    assert_eq!(app.rows[Tab::Limits.index()], 0);
    key(&mut app, KeyCode::Char('q'));
    assert!(app.quit);
}

#[test]
fn models_screen_runs_doctor_offline_and_online_on_request() {
    let (_d, mut app) = fixture("", "");
    let out = render(&mut app);
    assert!(out.contains("fake check, online=false"), "{out}");
    assert!(out.contains("offline: no network"), "{out}");
    key(&mut app, KeyCode::Char('o'));
    let out = render(&mut app);
    assert!(out.contains("fake check, online=true"), "{out}");
    assert!(out.contains("never a model call"), "{out}");
}

#[test]
fn tightening_applies_directly_and_is_audited() {
    let (_d, mut app) = fixture("", "");
    select(&mut app, "limits.frontier_usd");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(&app.mode, Mode::Edit { buffer, .. } if buffer == "5.0"));
    retype(&mut app, "2.0");
    assert!(render(&mut app).contains("set limits.frontier_usd: 2.0"));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Normal), "{}", app.status());
    let cfg = reload(&app);
    assert_eq!(cfg.float("limits.frontier_usd").unwrap(), 2.0);
    assert_eq!(cfg.origin("limits.frontier_usd"), Some(Origin::Owner));
    assert!(
        matches!(&config_audit(&app)[..], [AuditEvent::ConfigChange { key, file, weakens: None, confirmed: false, .. }]
            if key == "limits.frontier_usd" && file == "owner")
    );
    assert!(render(&mut app).contains("recorded in the config audit log"));
}

#[test]
fn loosening_shows_the_diff_and_cancel_leaves_everything_unchanged() {
    let (_d, mut app) = fixture("", "");
    select(&mut app, "sensitivity.detect_pii");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Confirm(_)));
    let out = render(&mut app);
    for want in [
        "confirm loosening",
        "This change loosens privacy.",
        "- true",
        "+ false",
        "weakens: Personal-data detectors",
        "y apply",
    ] {
        assert!(out.contains(want), "missing {want:?}:\n{out}");
    }
    key(&mut app, KeyCode::Char('x'));
    assert!(
        matches!(app.mode, Mode::Confirm(_)),
        "other keys do not confirm"
    );
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::Normal));
    assert!(app.status().contains("not applied"));
    assert!(!app.paths.owner.exists());
    assert!(config_audit(&app).is_empty());
    assert!(reload(&app).bool("sensitivity.detect_pii").unwrap());
}

#[test]
fn confirmed_loosening_applies_and_records_what_it_weakened() {
    let (_d, mut app) = fixture("", "");
    select(&mut app, "sensitivity.globs");
    key(&mut app, KeyCode::Enter);
    let Mode::Edit { buffer, .. } = &app.mode else {
        panic!("not editing")
    };
    // Drop one entry: removing a glob loosens.
    let without = buffer.replace("\"*.csv\", ", "");
    app.mode = Mode::Edit {
        key: "sensitivity.globs",
        buffer: without,
        append: false,
    };
    key(&mut app, KeyCode::Enter);
    let out = render(&mut app);
    assert!(out.contains("- \"*.csv\""), "{out}");
    key(&mut app, KeyCode::Char('y'));
    assert!(app.status().contains("(confirmed)"), "{}", app.status());
    let globs = reload(&app).list("sensitivity.globs").unwrap();
    assert!(!globs.contains(&"*.csv".to_owned()));
    let events = config_audit(&app);
    assert!(
        matches!(&events[..], [AuditEvent::ConfigChange { weakens: Some(w), confirmed: true, .. }] if w.contains("never reaches the frontier")),
        "{events:?}"
    );
    assert!(matches!(
        audit::verify(&app.paths.config_audit()).unwrap(),
        audit::Verification::Intact { records: 1 }
    ));
}

#[test]
fn project_scope_refuses_owner_only_keys_and_loosening() {
    let (_d, mut app) = fixture("", "");
    key(&mut app, KeyCode::Char('p'));
    assert!(render(&mut app).contains("project config ("));
    select(&mut app, "frontier.base_url");
    key(&mut app, KeyCode::Enter);
    typed(&mut app, "x");
    key(&mut app, KeyCode::Enter);
    assert!(
        app.status()
            .contains("can only be set in the owner's config"),
        "{}",
        app.status()
    );
    select(&mut app, "sensitivity.detect_pii");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Normal), "no confirm offered");
    assert!(
        app.status().contains("would loosen privacy"),
        "{}",
        app.status()
    );
    assert!(!app.paths.project.exists(), "nothing written");
    assert!(config_audit(&app).is_empty());

    // Tightening is allowed and shows its origin.
    select(&mut app, "sensitivity.globs");
    key(&mut app, KeyCode::Char('a'));
    typed(&mut app, "reports/**");
    key(&mut app, KeyCode::Enter);
    let cfg = reload(&app);
    assert!(
        cfg.list("sensitivity.globs")
            .unwrap()
            .contains(&"reports/**".to_owned())
    );
    assert_eq!(cfg.origin("sensitivity.globs"), Some(Origin::Project));
    assert!(render(&mut app).contains("from: project, file"));
    assert!(
        matches!(&config_audit(&app)[..], [AuditEvent::ConfigChange { file, .. }] if file == "project")
    );
}

#[test]
fn edits_are_validated_and_cancellable() {
    let (_d, mut app) = fixture("", "");
    select(&mut app, "limits.wall_clock_minutes");
    key(&mut app, KeyCode::Enter);
    retype(&mut app, "0");
    key(&mut app, KeyCode::Enter);
    assert!(app.status().contains("refused"), "{}", app.status());
    select(&mut app, "frontier.reasoning_effort");
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        reload(&app).str("frontier.reasoning_effort").unwrap(),
        "high"
    );
    select(&mut app, "local.model");
    key(&mut app, KeyCode::Enter);
    typed(&mut app, "-2");
    key(&mut app, KeyCode::Esc);
    assert!(app.status().contains("cancelled"));
    assert_eq!(reload(&app).str("local.model").unwrap(), "omlx-coding");
}

#[test]
fn sensitivity_tester_explains_matches() {
    let (_d, mut app) = fixture("", "");
    app.enter_tab(Tab::Sensitivity);
    key(&mut app, KeyCode::Char('t'));
    typed(&mut app, "data/users.csv");
    let out = render(&mut app);
    assert!(out.contains("sensitive: yes"), "{out}");
    assert!(out.contains("\"data/**\""), "{out}");
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('t'));
    for _ in 0.."data/users.csv".len() {
        key(&mut app, KeyCode::Backspace);
    }
    typed(&mut app, "src/main.rs");
    let out = render(&mut app);
    assert!(
        out.contains("sensitive: no") && out.contains("IP level: open"),
        "{out}"
    );
}

fn git_workspace(ws: &Path) {
    let git = duet_git::Git::locate().unwrap();
    std::process::Command::new(git.binary())
        .args(["init", "-q"])
        .current_dir(ws)
        .status()
        .unwrap();
    std::fs::create_dir_all(ws.join("src/pricing")).unwrap();
    std::fs::write(
        ws.join("src/pricing/engine.rs"),
        "/// Discount.\npub fn discount(q: u32) -> u32 {\n    q * 7 / 100\n}\n",
    )
    .unwrap();
    std::fs::write(ws.join("README.md"), "# ws\n").unwrap();
    std::fs::write(ws.join(".gitignore"), "ignored.txt\n").unwrap();
    std::fs::write(ws.join("ignored.txt"), "x").unwrap();
}

#[test]
fn ip_screen_marks_paths_and_previews_the_skeleton() {
    let (_d, mut app) = fixture("", "");
    git_workspace(&app.paths.workspace.clone());
    key(&mut app, KeyCode::Char('3'));
    let out = render(&mut app);
    assert!(out.contains("README.md") && out.contains("src/"), "{out}");
    assert!(!out.contains("ignored.txt"), ".gitignore applies:\n{out}");
    let pick = |app: &mut App, path: &str| {
        app.ip.selected = app.ip.rows().iter().position(|r| r.path == path).unwrap();
    };
    pick(&mut app, "src");
    key(&mut app, KeyCode::Enter);
    pick(&mut app, "src/pricing");
    key(&mut app, KeyCode::Enter);
    pick(&mut app, "src/pricing/engine.rs");
    let out = render(&mut app);
    assert!(out.contains("pub fn discount(q: u32) -> u32"), "{out}");
    assert!(out.contains("⟨body of fn discount⟩"), "{out}");
    assert!(!out.contains("q * 7 / 100"), "body withheld:\n{out}");

    key(&mut app, KeyCode::Char('p'));
    key(&mut app, KeyCode::Char('i'));
    let cfg = reload(&app);
    assert_eq!(
        cfg.list("ip.interface_only").unwrap(),
        vec!["src/pricing/engine.rs"]
    );
    assert!(render(&mut app).contains("[interface-only]"));
    pick(&mut app, "src/pricing");
    key(&mut app, KeyCode::Char('s'));
    assert_eq!(
        reload(&app).list("ip.sealed").unwrap(),
        vec!["src/pricing/**"]
    );
    assert!(render(&mut app).contains("pricing/  [sealed]"));
    pick(&mut app, "src/pricing/engine.rs");
    assert!(render(&mut app).contains("is sealed: the frontier sees only that it exists"));
    pick(&mut app, "src/pricing");
    // Unmarking loosens: the project file refuses it.
    key(&mut app, KeyCode::Char('u'));
    assert!(
        app.status().contains("would loosen privacy"),
        "{}",
        app.status()
    );
    assert_eq!(
        reload(&app).list("ip.sealed").unwrap(),
        vec!["src/pricing/**"]
    );
}

fn audited_run(app: &App, id: &str) -> std::path::PathBuf {
    let ws = &app.paths.workspace;
    let log = ws.join(".duet/audit").join(format!("{id}.jsonl"));
    let mut a = AuditLog::open_anchored(&log, &run_anchors(&app.paths.state, ws, id)).unwrap();
    a.event(AuditEvent::RunStart {
        mode: "hybrid".into(),
        boundary: true,
    })
    .unwrap();
    a.append(
        "https://frontier.example/v1",
        "m",
        json!({"messages": [{"role": "user", "content": "fix the bug, key is ⟨secret:1⟩"}]}),
        vec!["1 secret replaced".into()],
    )
    .unwrap();
    a.event(AuditEvent::BlockedSend {
        check: "canary".into(),
    })
    .unwrap();
    log
}

#[test]
fn audit_screen_lists_records_and_verifies() {
    let (_d, mut app) = fixture("", "");
    let log = audited_run(&app, "20260924-120000-abcdef");
    key(&mut app, KeyCode::Char('6'));
    let out = render(&mut app);
    for want in [
        "20260924-120000-abcdef",
        "run_start",
        "request",
        "blocked_send",
    ] {
        assert!(out.contains(want), "missing {want}:\n{out}");
    }
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Down);
    let out = render(&mut app);
    assert!(
        out.contains("user: fix the bug, key is ⟨secret:1⟩"),
        "{out}"
    );
    assert!(out.contains("1 secret replaced"), "{out}");
    key(&mut app, KeyCode::Char('v'));
    let out = render(&mut app);
    assert!(out.contains("chain intact: 3 records"), "{out}");
    assert!(out.contains("anchor matches"), "{out}");

    // Truncating the log is caught by the anchor.
    let text = std::fs::read_to_string(&log).unwrap();
    let kept: Vec<&str> = text.lines().take(2).collect();
    std::fs::write(&log, format!("{}\n", kept.join("\n"))).unwrap();
    key(&mut app, KeyCode::Char('v'));
    assert!(render(&mut app).contains("REWRITTEN OR TRUNCATED"));
}

#[test]
fn run_screen_follows_the_transcript() {
    let (_d, mut app) = fixture("", "");
    let id = "20260924-130000-123456";
    let run_dir = app.paths.workspace.join(".duet/runs").join(id);
    let t = Transcript::open(&run_dir).unwrap();
    t.append(&Entry::Start {
        objective: "Fix the failing test".into(),
        mode: "hybrid".into(),
        frontier_model: "glm".into(),
    })
    .unwrap();
    t.append(&Entry::Item {
        item: Item::Assistant {
            text: "Reading the log first.".into(),
            reasoning: None,
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                arguments: serde_json::Map::new(),
                raw_arguments: r#"{"path":"logs/app.log"}"#.into(),
            }],
        },
    })
    .unwrap();
    t.append(&Entry::Shown {
        call_id: "c1".into(),
        class: ViewClass::HandleSummary,
    })
    .unwrap();
    audited_run(&app, id);
    key(&mut app, KeyCode::Char('7'));
    let out = render(&mut app);
    for want in [
        "start (hybrid, glm): Fix the failing test",
        "turn 1: Reading the log first.",
        "-> read_file {\"path\":\"logs/app.log\"}",
        "c1: shown as HandleSummary",
        "request #2: 1 secret replaced",
        "blocked send: canary",
    ] {
        assert!(out.contains(want), "missing {want}:\n{out}");
    }
    t.append(&Entry::Usage {
        turn: 1,
        usage: Usage::default(),
        cost_usd: 0.0123,
        interventions: vec!["2 emails tokenized".into()],
    })
    .unwrap();
    app.tick();
    let out = render(&mut app);
    assert!(out.contains("turn 1 cost $0.0123"), "{out}");
    assert!(out.contains("turn 1: 2 emails tokenized"), "{out}");
}

#[test]
fn empty_workspace_screens_render() {
    let (_d, mut app) = fixture("", "");
    for tab in Tab::ALL {
        app.enter_tab(tab);
        let out = screen(&mut app, 80, 24);
        assert!(out.contains(tab.title()), "{tab:?}:\n{out}");
    }
    app.enter_tab(Tab::Run);
    assert!(render(&mut app).contains("run: none yet"));
    app.enter_tab(Tab::Audit);
    assert!(render(&mut app).contains("runs: none yet"));
}
