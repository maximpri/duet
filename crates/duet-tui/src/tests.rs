// SPDX-License-Identifier: GPL-3.0-or-later
//! Screen tests on ratatui's `TestBackend`: every screen renders, navigation,
//! the edit flows (tightening, loosening confirm and cancel, project-scope
//! refusals) and the registry round trip. No model is contacted.

use crate::app::{App, DoctorLine, Mode, Paths, Tab};
use crate::settings::{keys, screen_of};
use crate::{CacheReport, LocalServer};
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

fn services() -> crate::Services {
    crate::Services {
        doctor: Box::new(|online| {
            vec![DoctorLine {
                status: "pass".into(),
                name: "config".into(),
                detail: format!("fake check, online={online}"),
                fix: None,
            }]
        }),
        detect: std::sync::Arc::new(|| {
            vec![
                LocalServer {
                    base_url: "http://127.0.0.1:11434/v1".into(),
                    backend: "Ollama".into(),
                    models: vec!["qwen3:8b".into(), "coder:30b".into()],
                },
                LocalServer {
                    base_url: "http://127.0.0.1:8080/v1".into(),
                    backend: "llama.cpp server".into(),
                    models: vec![],
                },
            ]
        }),
        cache_probe: std::sync::Arc::new(|| {
            Ok(CacheReport {
                base_url: "http://127.0.0.1:11434/v1".into(),
                model: "qwen3:8b".into(),
                prompt_tokens: 1500,
                first_cached: 0,
                second_cached: 1408,
                first_seconds: 2.5,
                second_seconds: 0.4,
                unreported: false,
            })
        }),
        duet: None,
    }
}

/// Waits for background jobs (detection, cache probe) to finish.
fn settle(app: &mut App) {
    for _ in 0..500 {
        app.tick();
        if !app.busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("background job did not finish");
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
    let app = App::new(paths, services()).unwrap();
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
        let out = screen(&mut app, 200, 80);
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
fn a_sub_agents_steps_show_indented_under_its_id() {
    let assistant = |text: &str| Entry::Item {
        item: Item::Assistant {
            text: text.into(),
            reasoning: None,
            replay: None,
            tool_calls: vec![],
        },
    };
    let nested = |entry| Entry::Subagent {
        child: "a1".into(),
        entry: Box::new(entry),
    };
    let entries = vec![
        assistant("Delegating."),
        Entry::SubagentStart {
            child: "a1".into(),
            call_id: "c1".into(),
            mode: "write".into(),
            task: "Fix the export.".into(),
            paths: vec!["src/export/**".into()],
            journal_next: 1,
        },
        nested(assistant("Reading the module.")),
        nested(Entry::Shown {
            call_id: "c2".into(),
            class: ViewClass::HandleSummary,
        }),
        nested(assistant("Editing it.")),
        Entry::SubagentEnd {
            child: "a1".into(),
            call_id: "c1".into(),
            terminal: duet_agent::Terminal::Completed {
                summary: "fixed".into(),
            },
            requests: 2,
            cost_usd: 0.002,
            seconds: 3.0,
            written: vec!["src/export/mod.rs".into()],
            journal_end: 3,
        },
        assistant("Done."),
    ];
    let (feed, withheld) = crate::runs::feeds(&entries, &[]);
    assert_eq!(
        feed,
        [
            "turn 1: Delegating.",
            "sub-agent a1 (write) started (may write src/export/**): Fix the export.",
            "  [a1] turn 1: Reading the module.",
            "  [a1] turn 2: Editing it.",
            "sub-agent a1 ended completed after 2 request(s), $0.0020; 1 file(s) written",
            "turn 2: Done.",
        ]
    );
    assert_eq!(withheld, ["[a1] c2: shown as HandleSummary"]);
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
            replay: None,
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

#[test]
fn a_terminal_without_a_usable_size_is_refused_with_the_minimum() {
    let (w, h) = crate::MIN_SIZE;
    assert_eq!(crate::check_size(w, h), Ok(()));
    assert_eq!(crate::check_size(200, 60), Ok(()));
    for (width, height) in [(0, 0), (w - 1, h), (w, h - 1), (40, 10)] {
        let message = crate::check_size(width, height).unwrap_err();
        assert_eq!(
            message,
            format!("terminal too small: need at least {w}x{h}, this one is {width}x{height}")
        );
    }
    // A terminal shrunk while running shows the message instead of a clipped screen.
    let (_d, mut app) = fixture("", "");
    let out = screen(&mut app, 60, 12);
    assert!(
        out.contains("terminal too small: need at least 80x24"),
        "{out}"
    );
    assert!(!out.contains("edits go to"), "{out}");
}

#[test]
fn models_screen_detects_local_servers_and_uses_one_through_the_audited_path() {
    let (_d, mut app) = fixture("", "");
    key(&mut app, KeyCode::Char('l'));
    settle(&mut app);
    let out = render(&mut app);
    for want in [
        "Ollama at http://127.0.0.1:11434/v1",
        "> qwen3:8b",
        "coder:30b",
        "llama.cpp server at http://127.0.0.1:8080/v1",
        "lists no models",
    ] {
        assert!(out.contains(want), "missing {want:?}:\n{out}");
    }
    assert!(
        app.status().contains("2 local server(s)"),
        "{}",
        app.status()
    );
    key(&mut app, KeyCode::Char(']'));
    assert!(render(&mut app).contains("> coder:30b"));
    key(&mut app, KeyCode::Char(']'));
    assert_eq!(app.models.pick, 1, "stays on the last choice");
    // The project file never takes the endpoint.
    key(&mut app, KeyCode::Char('p'));
    key(&mut app, KeyCode::Char('u'));
    assert!(app.status().contains("owner-only"), "{}", app.status());
    key(&mut app, KeyCode::Char('p'));
    // Changing the endpoint loosens privacy: it waits for y.
    key(&mut app, KeyCode::Char('u'));
    assert!(matches!(app.mode, Mode::Confirm(_)));
    assert!(render(&mut app).contains("+ \"http://127.0.0.1:11434/v1\""));
    key(&mut app, KeyCode::Char('y'));
    let cfg = reload(&app);
    assert_eq!(
        cfg.str("local.base_url").unwrap(),
        "http://127.0.0.1:11434/v1"
    );
    assert_eq!(cfg.str("local.model").unwrap(), "coder:30b");
    let events = config_audit(&app);
    assert!(
        matches!(&events[..], [
            AuditEvent::ConfigChange { key: k1, confirmed: true, .. },
            AuditEvent::ConfigChange { key: k2, .. },
        ] if k1 == "local.base_url" && k2 == "local.model"),
        "{events:?}"
    );
}

#[test]
fn cancelling_the_endpoint_change_also_drops_the_model_change() {
    let (_d, mut app) = fixture("", "");
    key(&mut app, KeyCode::Char('l'));
    settle(&mut app);
    key(&mut app, KeyCode::Char('u'));
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::Normal));
    assert!(!app.paths.owner.exists());
    assert!(config_audit(&app).is_empty());
}

#[test]
fn models_screen_runs_the_cache_probe_only_on_request() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (_d, mut app) = fixture("", "");
    let counted = calls.clone();
    let inner = app.services_mut().cache_probe.clone();
    app.services_mut().cache_probe = std::sync::Arc::new(move || {
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        inner()
    });
    for tab in Tab::ALL {
        app.enter_tab(tab);
        render(&mut app);
    }
    app.tick();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    app.enter_tab(Tab::Models);
    key(&mut app, KeyCode::Char('c'));
    settle(&mut app);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let out = render(&mut app);
    for want in [
        "qwen3:8b at http://127.0.0.1:11434/v1",
        "first request:       0 cached of 1500 prompt tokens, 2.5s",
        "second request:   1408 cached (94%), 0.4s",
        "the server reuses its prompt cache",
    ] {
        assert!(out.contains(want), "missing {want:?}:\n{out}");
    }
    // A failure is shown, not fatal.
    app.services_mut().cache_probe = std::sync::Arc::new(|| Err("connection refused".into()));
    key(&mut app, KeyCode::Char('c'));
    settle(&mut app);
    assert!(render(&mut app).contains("connection refused"));
    assert!(app.status().contains("cache probe failed"));
    // d returns to the doctor panel.
    key(&mut app, KeyCode::Char('d'));
    assert!(render(&mut app).contains("fake check, online=false"));
}

#[test]
fn sensitivity_text_tester_shows_replacements_including_custom_patterns() {
    let (_d, mut app) = fixture("", "");
    select(&mut app, "sensitivity.custom_patterns");
    key(&mut app, KeyCode::Char('a'));
    typed(&mut app, "CUST-[0-9]{6}");
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        reload(&app).list("sensitivity.custom_patterns").unwrap(),
        vec!["CUST-[0-9]{6}"]
    );
    // An invalid pattern is refused by the registry.
    key(&mut app, KeyCode::Char('a'));
    typed(&mut app, "CUST-(");
    key(&mut app, KeyCode::Enter);
    assert!(
        app.status().contains("not a valid regular expression"),
        "{}",
        app.status()
    );
    key(&mut app, KeyCode::Char('s'));
    typed(&mut app, "refund CUST-004211 for kim@corp.net");
    let out = screen(&mut app, 200, 60);
    for want in [
        "sent as: refund ⟨data:1⟩ for ⟨email:1⟩",
        "CUST-004211 -> ⟨data:1⟩ (custom pattern \"CUST-[0-9]{6}\")",
        "kim@corp.net -> ⟨email:1⟩ (email detector)",
    ] {
        assert!(out.contains(want), "missing {want:?}:\n{out}");
    }
    key(&mut app, KeyCode::Esc);
    for _ in 0..40 {
        key(&mut app, KeyCode::Backspace);
    }
    // Backspace after Esc edits nothing; s resumes typing.
    key(&mut app, KeyCode::Char('s'));
    for _ in 0..40 {
        key(&mut app, KeyCode::Backspace);
    }
    typed(&mut app, "fn main() {}");
    assert!(screen(&mut app, 200, 60).contains("nothing here would be replaced"));
    // Removing the pattern loosens: it needs a confirmation.
    key(&mut app, KeyCode::Esc);
    select(&mut app, "sensitivity.custom_patterns");
    key(&mut app, KeyCode::Enter);
    retype(&mut app, "[]");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Confirm(_)));
}

fn make_runs(app: &App, ids: &[&str]) {
    for id in ids {
        let dir = app.paths.workspace.join(".duet/runs").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("vault.json"), "{}").unwrap();
    }
}

#[test]
fn data_screen_purges_after_confirmation() {
    let (_d, mut app) = fixture("", "");
    make_runs(&app, &["20260901-000000-aaaaaa", "20260902-000000-bbbbbb"]);
    let runs = app.paths.workspace.join(".duet/runs");
    app.enter_tab(Tab::Data);
    // Nothing is older than the retention period.
    key(&mut app, KeyCode::Char('x'));
    assert!(matches!(app.mode, Mode::Normal));
    assert_eq!(app.status(), "nothing to purge");
    key(&mut app, KeyCode::Char('X'));
    let out = render(&mut app);
    for want in [
        "confirm purge",
        "Delete the raw data of 2 run(s): every run.",
        "20260901-000000-aaaaaa",
        "audit logs stay",
    ] {
        assert!(out.contains(want), "missing {want:?}:\n{out}");
    }
    key(&mut app, KeyCode::Char('n'));
    assert!(app.status().contains("nothing deleted"));
    assert_eq!(std::fs::read_dir(&runs).unwrap().count(), 2);
    // A run in progress holds the workspace lock: nothing is deleted.
    let lock = duet_fs::lock::WorkspaceLock::acquire(&app.paths.workspace).unwrap();
    key(&mut app, KeyCode::Char('X'));
    key(&mut app, KeyCode::Char('y'));
    assert!(
        app.status().contains("a run is in progress"),
        "{}",
        app.status()
    );
    assert_eq!(std::fs::read_dir(&runs).unwrap().count(), 2);
    drop(lock);
    // A child another test forks in this process can hold a copy of the lock
    // until it execs; retry briefly.
    for _ in 0..100 {
        key(&mut app, KeyCode::Char('X'));
        key(&mut app, KeyCode::Char('y'));
        if !app.status().contains("in progress") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        app.status().contains("purged the raw data of 2 run(s)"),
        "{}",
        app.status()
    );
    assert_eq!(std::fs::read_dir(&runs).unwrap().count(), 0);
}

/// A run in a git workspace that edits, creates and writes a sensitive file
/// through the write journal, plus a file derived by a sensitive command.
fn run_with_writes(app: &App, id: &str) -> std::path::PathBuf {
    use duet_agent::journal::WriteJournal;
    use duet_fs::Precondition;
    let ws = app.paths.workspace.clone();
    git_workspace(&ws);
    std::fs::write(
        ws.join("src/lib.rs"),
        (1..=30).map(|i| format!("line {i}\n")).collect::<String>(),
    )
    .unwrap();
    std::fs::write(ws.join(".env"), "API_TOKEN=before-value-123\n").unwrap();
    let run_dir = ws.join(".duet/runs").join(id);
    let t = Transcript::open(&run_dir).unwrap();
    t.append(&Entry::Start {
        objective: "Tidy the library".into(),
        mode: "hybrid".into(),
        frontier_model: "glm".into(),
    })
    .unwrap();
    let mut j = WriteJournal::open(&run_dir).unwrap();
    let edited: String = (1..=30)
        .map(|i| match i {
            3 => "line three\n".to_owned(),
            20 => "line 20\ninserted after 20\n".to_owned(),
            _ => format!("line {i}\n"),
        })
        .collect();
    j.write(
        &ws,
        Path::new("src/lib.rs"),
        edited.as_bytes(),
        &Precondition::Any,
    )
    .unwrap();
    j.write(
        &ws,
        Path::new("src/new.rs"),
        b"pub fn fresh() {}\n",
        &Precondition::Any,
    )
    .unwrap();
    j.write(
        &ws,
        Path::new(".env"),
        b"API_TOKEN=after-value-456\n",
        &Precondition::Any,
    )
    .unwrap();
    std::fs::create_dir_all(ws.join("out")).unwrap();
    std::fs::write(ws.join("out/report.txt"), "customer kim@corp.net owes 12\n").unwrap();
    let log = ws.join(".duet/audit").join(format!("{id}.jsonl"));
    let mut a = AuditLog::open_anchored(&log, &run_anchors(&app.paths.state, &ws, id)).unwrap();
    a.event(AuditEvent::SensitiveCommand {
        command: "python report.py".into(),
        exit_code: Some(0),
        derived_files: vec!["out/report.txt".into()],
    })
    .unwrap();
    run_dir
}

#[test]
fn run_view_lists_changed_files_and_shows_numbered_diffs() {
    let (_d, mut app) = fixture("", "");
    let id = "20260925-090000-d1ff00";
    run_with_writes(&app, id);
    key(&mut app, KeyCode::Char('7'));
    let out = render(&mut app);
    for want in [
        "start (hybrid, glm): Tidy the library",
        "changed files 4 (+3 -1)",
        "edit src/lib.rs  +2  -1",
        "new  src/new.rs  +1",
        "held .env  held locally",
        "held out/report.txt  held locally",
        "sensitive command held locally: python report.py",
    ] {
        assert!(out.contains(want), "missing {want:?}:\n{out}");
    }
    // Following: the file written last is selected; .env is held, so no content.
    assert!(
        out.contains(".env: it matches the sensitivity rules"),
        "{out}"
    );
    for secret in ["before-value-123", "after-value-456", "kim@corp.net"] {
        assert!(!out.contains(secret), "{secret} shown:\n{out}");
    }
    // Focus the side panel and pick src/lib.rs.
    key(&mut app, KeyCode::Right);
    assert_eq!(app.runs.focus, crate::runs::Focus::Side);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Up);
    assert!(!app.runs.follow, "picking a file stops following");
    let out = render(&mut app);
    for want in [
        "src/lib.rs (edited; rows 1-",
        "┄ from line 1",
        " 3    - line 3",
        "    3 + line three",
        "┄ from line 18",
        "20 20   line 20",
        "   21 + inserted after 20",
        "21 22   line 21",
    ] {
        assert!(out.contains(want), "missing {want:?}:\n{out}");
    }
    // Scrolling the diff: a page, then a line back.
    key(&mut app, KeyCode::PageDown);
    assert!(app.runs.diff_scroll > 0);
    let at = app.runs.diff_scroll;
    key(&mut app, KeyCode::Char('K'));
    assert_eq!(app.runs.diff_scroll, at - 1);
    // The new file.
    key(&mut app, KeyCode::Down);
    let out = render(&mut app);
    assert!(out.contains("src/new.rs (created"), "{out}");
    assert!(out.contains(" 1 + pub fn fresh() {}"), "{out}");
    // The derived file is held too.
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    let out = render(&mut app);
    assert!(
        out.contains("out/report.txt: a command that could read sensitive data"),
        "{out}"
    );
    assert!(!out.contains("kim@corp.net"));
    // Back to the main panel: arrows scroll the feed again.
    key(&mut app, KeyCode::Left);
    assert_eq!(app.runs.focus, crate::runs::Focus::Main);
    // The smallest terminal still shows both panels.
    let small = screen(&mut app, 80, 24);
    assert!(small.contains("changed files 4"), "{small}");
    assert!(small.contains("withheld and blocked"), "{small}");
}

#[test]
fn run_view_follows_edits_as_they_happen_and_reads_finished_runs() {
    use duet_agent::journal::WriteJournal;
    use duet_fs::Precondition;
    let (_d, mut app) = fixture("", "");
    let id = "20260925-091500-f011a0";
    let run_dir = run_with_writes(&app, id);
    let ws = app.paths.workspace.clone();
    key(&mut app, KeyCode::Char('7'));
    render(&mut app);
    // The run writes README.md: the view moves to it and shows the change.
    let mut j = WriteJournal::open(&run_dir).unwrap();
    j.write(
        &ws,
        Path::new("README.md"),
        b"# ws\n\nUsage notes.\n",
        &Precondition::Any,
    )
    .unwrap();
    app.tick();
    let out = render(&mut app);
    assert!(out.contains("README.md (edited"), "{out}");
    assert!(out.contains(" 3 + Usage notes."), "{out}");
    // Another edit to the same file updates its diff in place.
    j.write(
        &ws,
        Path::new("README.md"),
        b"# ws\n\nUsage notes, revised.\n",
        &Precondition::Any,
    )
    .unwrap();
    app.tick();
    let out = render(&mut app);
    assert!(out.contains("+ Usage notes, revised."), "{out}");
    assert!(!out.contains("+ Usage notes.\n"), "{out}");
    // A write that restores the original shows as unchanged.
    j.write(&ws, Path::new("README.md"), b"# ws\n", &Precondition::Any)
        .unwrap();
    app.tick();
    let out = render(&mut app);
    assert!(out.contains("same README.md"), "{out}");
    // The run ends; the view reads the same way (read-only).
    Transcript::open(&run_dir)
        .unwrap()
        .append(&Entry::End {
            terminal: duet_agent::Terminal::Completed {
                summary: "done".into(),
            },
        })
        .unwrap();
    app.tick();
    let out = render(&mut app);
    assert!(out.contains("end: "), "{out}");
    assert!(out.contains("edit src/lib.rs  +2  -1"), "{out}");
    let status = duet_git::Git::locate()
        .unwrap()
        .run(&ws, &["status", "--porcelain", "--", "src"], &[], None)
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&status).contains("A "),
        "the viewer staged nothing"
    );
}

/// A stand-in for the `duet` binary: records its arguments, announces a run
/// the way `duet run` does, creates the run's transcript and exits with `code`.
fn fake_duet(dir: &Path, code: i32, announce: bool) -> std::path::PathBuf {
    let script = dir.join("fake-duet");
    let run = if announce {
        r#"echo "run 20260925-120000-c0ffee (Hybrid)" >&2
mkdir -p "$2/.duet/runs/20260925-120000-c0ffee"
printf '%s\n' '{"kind":"start","objective":"from the tui","mode":"hybrid","frontier_model":"glm"}' > "$2/.duet/runs/20260925-120000-c0ffee/transcript.jsonl"
"#
    } else {
        "echo \"no local model server answered\" >&2\n"
    };
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"{}\"\n{run}exit {code}\n",
            dir.join("args").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    script
}

fn wait_for_launch(app: &mut App) {
    for _ in 0..500 {
        app.tick();
        if app.launched.as_ref().is_some_and(|l| l.exit.is_some()) {
            app.tick();
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the launched process did not finish");
}

#[test]
fn starting_a_run_launches_duet_run_and_opens_the_run_view() {
    let (d, mut app) = fixture("", "");
    app.services_mut().duet = Some(fake_duet(d.path(), 0, true));
    key(&mut app, KeyCode::Char('7'));
    key(&mut app, KeyCode::Char('o'));
    let out = render(&mut app);
    assert!(out.contains("start a one-shot run (duet run)"), "{out}");
    assert!(out.contains("> hybrid"), "hybrid is the default:\n{out}");
    key(&mut app, KeyCode::Enter);
    assert!(app.status().contains("objective first"));
    typed(&mut app, "fix the flaky test");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Normal));
    wait_for_launch(&mut app);
    let args = std::fs::read_to_string(d.path().join("args")).unwrap();
    let ws = app.paths.workspace.display().to_string();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        vec![
            "--workspace",
            &ws,
            "run",
            "--mode",
            "hybrid",
            "--",
            "fix the flaky test"
        ]
    );
    assert_eq!(app.tab(), Tab::Run);
    assert_eq!(app.runs.runs[app.runs.selected], "20260925-120000-c0ffee");
    let out = render(&mut app);
    assert!(out.contains("run 20260925-120000-c0ffee"), "{out}");
    assert!(out.contains("start (hybrid, glm): from the tui"), "{out}");
    assert!(
        app.status().contains("completed (exit code 0)"),
        "{}",
        app.status()
    );
    // The child's output went to a private log, not the terminal.
    let log = &app.launched.as_ref().unwrap().log;
    assert!(log.starts_with(app.paths.workspace.join(".duet/tmp")));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(log).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn passthrough_needs_the_no_privacy_acknowledgement() {
    let (d, mut app) = fixture("", "");
    app.services_mut().duet = Some(fake_duet(d.path(), 0, true));
    app.enter_tab(Tab::Run);
    key(&mut app, KeyCode::Char('o'));
    typed(&mut app, "baseline");
    key(&mut app, KeyCode::Up);
    assert!(render(&mut app).contains("> passthrough"));
    key(&mut app, KeyCode::Enter);
    let out = render(&mut app);
    assert!(out.contains("acknowledge: no privacy"), "{out}");
    assert!(out.contains("PRIVACY BOUNDARY OFF"), "{out}");
    key(&mut app, KeyCode::Char('x'));
    assert!(
        matches!(app.mode, Mode::AckPassthrough { .. }),
        "only y confirms"
    );
    key(&mut app, KeyCode::Char('n'));
    assert!(app.status().contains("boundary stays on"));
    assert!(app.launched.is_none());
    assert!(!d.path().join("args").exists(), "nothing was started");
    key(&mut app, KeyCode::Char('o'));
    typed(&mut app, "baseline");
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('y'));
    wait_for_launch(&mut app);
    let args = std::fs::read_to_string(d.path().join("args")).unwrap();
    assert!(
        args.contains("--mode\npassthrough\n--no-privacy\n--\nbaseline"),
        "{args}"
    );
    // The launcher itself refuses passthrough without the acknowledgement.
    assert!(
        crate::launch::spawn(
            &d.path().join("fake-duet"),
            &app.paths.workspace,
            "x",
            crate::launch::RunMode::Passthrough,
            false
        )
        .is_err()
    );
}

#[test]
fn a_run_that_cannot_start_reports_why_and_approval_refuses_launching() {
    let (d, mut app) = fixture("", "");
    app.services_mut().duet = Some(fake_duet(d.path(), 2, false));
    app.enter_tab(Tab::Run);
    key(&mut app, KeyCode::Char('o'));
    typed(&mut app, "anything");
    key(&mut app, KeyCode::Enter);
    wait_for_launch(&mut app);
    assert!(
        app.status()
            .contains("exited with code 2 before a run started: no local model server answered"),
        "{}",
        app.status()
    );
    // With operator approval on, runs need a terminal: the TUI refuses.
    let (d2, mut app) = fixture("[oversight]\napprove = \"risky\"\n", "");
    app.services_mut().duet = Some(fake_duet(d2.path(), 0, true));
    app.enter_tab(Tab::Run);
    key(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.mode, Mode::Normal));
    assert!(
        app.status().contains("oversight.approve is risky"),
        "{}",
        app.status()
    );
    // Without a duet executable there is nothing to start.
    let (_d3, mut app) = fixture("", "");
    app.enter_tab(Tab::Run);
    key(&mut app, KeyCode::Char('n'));
    assert!(app.status().contains("not available"), "{}", app.status());
}

/// A stand-in for `duet chat`: records its arguments, announces a session
/// (unless resuming), writes a transcript with one answered turn, and then
/// appends every line it reads from its input to `received` until the input
/// ends. It ignores SIGINT but records it.
fn fake_chat(dir: &Path) -> std::path::PathBuf {
    let script = dir.join("fake-chat");
    let transcript = r#"{"kind":"start","objective":"hello","mode":"hybrid","frontier_model":"glm"}
{"kind":"turn_start","exchange":1,"message":"hello","journal_next":1}
{"kind":"turn_end","exchange":1,"seconds":1.0,"end":{"state":"replied","message":"hi there"}}"#;
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
for a in "$@"; do printf '%s\n' "$a"; done > "{args}"
trap 'echo SIGINT >> "{received}"' INT
case "$*" in
*--resume*) ;;
*)
echo "session 20260925-130000-5e5510 (Hybrid)" >&2
mkdir -p "$2/.duet/runs/20260925-130000-5e5510"
cat > "$2/.duet/runs/20260925-130000-5e5510/transcript.jsonl" <<'EOF2'
{transcript}
EOF2
;;
esac
while IFS= read -r line; do printf '%s\n' "$line" >> "{received}"; done
"#,
            args = dir.join("args").display(),
            received = dir.join("received").display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    script
}

fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..3000 {
        if ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

fn received(d: &Path) -> String {
    std::fs::read_to_string(d.join("received")).unwrap_or_default()
}

#[test]
fn n_starts_a_session_and_its_input_box_sends_steers_and_stops() {
    let (d, mut app) = fixture("", "");
    app.services_mut().duet = Some(fake_chat(d.path()));
    app.enter_tab(Tab::Run);
    key(&mut app, KeyCode::Char('n'));
    let out = render(&mut app);
    assert!(out.contains("start a session (duet chat)"), "{out}");
    assert!(out.contains("first message:"), "{out}");
    typed(&mut app, "hello");
    key(&mut app, KeyCode::Enter);
    wait_until("the session id", || {
        app.tick();
        app.launched.as_ref().is_some_and(|l| l.run_id.is_some())
    });
    let args = std::fs::read_to_string(d.path().join("args")).unwrap();
    let ws = app.paths.workspace.display().to_string();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        vec![
            "--workspace",
            &ws,
            "chat",
            "--mode",
            "hybrid",
            "--",
            "hello"
        ]
    );
    wait_until("the conversation", || {
        app.tick();
        render(&mut app).contains("duet: hi there")
    });
    let out = render(&mut app);
    for want in [
        "session 20260925-130000-5e5510",
        "── turn 1 ──",
        "you> hello",
        "duet: hi there",
        "i type a message",
    ] {
        assert!(out.contains(want), "missing {want:?}:\n{out}");
    }
    // Type and send; the box stays open for the next message.
    key(&mut app, KeyCode::Char('i'));
    typed(&mut app, "fix the flaky test");
    assert!(render(&mut app).contains("> fix the flaky test"));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Message { .. }));
    wait_until("the message", || {
        received(d.path()).contains("fix the flaky test")
    });
    // /status is answered here, from the transcript; nothing is sent.
    typed(&mut app, "/status");
    key(&mut app, KeyCode::Enter);
    assert!(app.status().contains("1 turn(s)"), "{}", app.status());
    // Ctrl-C while typing stops the turn now (SIGINT), not the TUI.
    app.key(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(!app.quit);
    assert!(app.status().contains("interrupt sent"), "{}", app.status());
    // Esc leaves the box; s asks for a stop after the current step. (The
    // stand-in may only run its SIGINT trap once its read returns.)
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('s'));
    wait_until("the stop", || received(d.path()).contains("/stop"));
    wait_until("the interrupt", || received(d.path()).contains("SIGINT"));
    assert!(!received(d.path()).contains("/status"));
    // Closing the input ends the stand-in, as quitting the TUI would.
    app.launched.as_mut().unwrap().close_input();
    wait_until("the session to end", || {
        app.tick();
        app.launched.as_ref().is_some_and(|l| l.exit.is_some())
    });
}

#[test]
fn r_resumes_the_selected_session_and_refuses_runs_and_closed_sessions() {
    let (d, mut app) = fixture("", "");
    app.services_mut().duet = Some(fake_chat(d.path()));
    let runs = app.paths.workspace.join(".duet/runs");
    let t = |id: &str, entries: &[Entry]| {
        let tr = Transcript::open(&runs.join(id)).unwrap();
        for e in entries {
            tr.append(e).unwrap();
        }
    };
    let start = Entry::Start {
        objective: "o".into(),
        mode: "hybrid".into(),
        frontier_model: "glm".into(),
    };
    let turn = Entry::TurnStart {
        exchange: 1,
        message: "first".into(),
        journal_next: 1,
    };
    t("20260901-000000-aaaaaa", std::slice::from_ref(&start));
    t(
        "20260902-000000-bbbbbb",
        &[
            start.clone(),
            turn.clone(),
            Entry::Steered {
                exchange: 1,
                after_request: 2,
                messages: vec!["and the docs".into()],
            },
            Entry::TurnEnd {
                exchange: 1,
                seconds: 1.0,
                end: duet_agent::TurnEnd::Completed {
                    summary: "all done".into(),
                },
            },
            Entry::End {
                terminal: duet_agent::Terminal::Completed {
                    summary: "closed".into(),
                },
            },
        ],
    );
    t("20260903-000000-cccccc", &[start, turn]);
    app.enter_tab(Tab::Run);
    let select = |app: &mut App, id: &str| {
        let policy = app.policy();
        let ws = app.paths.workspace.clone();
        app.runs.show(id, &ws, &policy);
    };
    select(&mut app, "20260901-000000-aaaaaa");
    key(&mut app, KeyCode::Char('r'));
    assert!(app.status().contains("one-shot run"), "{}", app.status());
    select(&mut app, "20260902-000000-bbbbbb");
    let out = render(&mut app);
    for want in [
        "you> first",
        "you, delivered after request 2> and the docs",
        "duet finished: all done",
        "this session is closed",
    ] {
        assert!(out.contains(want), "missing {want:?}:\n{out}");
    }
    key(&mut app, KeyCode::Char('r'));
    assert!(app.status().contains("was closed"), "{}", app.status());
    assert!(app.launched.is_none());
    select(&mut app, "20260903-000000-cccccc");
    assert!(render(&mut app).contains("r resumes this session"));
    key(&mut app, KeyCode::Char('r'));
    assert!(matches!(app.mode, Mode::Message { .. }));
    wait_until("the arguments", || d.path().join("args").exists());
    wait_until("all arguments", || {
        std::fs::read_to_string(d.path().join("args"))
            .unwrap()
            .lines()
            .count()
            == 5
    });
    let args = std::fs::read_to_string(d.path().join("args")).unwrap();
    let ws = app.paths.workspace.display().to_string();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        vec![
            "--workspace",
            &ws,
            "chat",
            "--resume",
            "20260903-000000-cccccc"
        ]
    );
    typed(&mut app, "carry on");
    key(&mut app, KeyCode::Enter);
    wait_until("the message", || received(d.path()).contains("carry on"));
    app.launched.as_mut().unwrap().close_input();
}
