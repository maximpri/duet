// SPDX-License-Identifier: GPL-3.0-or-later
//! The client against the scripted server: in-process for the protocol, and
//! as a sandboxed process for confinement and crashes.

use declass_lsp::mock::{MockLauncher, MockOptions, detected};
use declass_lsp::position::{from_lsp, to_lsp};
use declass_lsp::{CallError, Lsp, LspError, Position, SandboxLauncher, Settings};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

const MAIN: &str = "/// Adds one.\nfn add_one(x: i32) -> i32 { x + 1 }\n\nfn main() {\n    let s = \"é😀\"; add_one(2);\n}\n";
const UTIL: &str = "fn helper() { add_one(3); }\n";

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let ws = d.path().canonicalize().unwrap().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/main.rs"), MAIN).unwrap();
    std::fs::write(ws.join("src/util.rs"), UTIL).unwrap();
    (d, ws)
}

fn settings() -> Settings {
    Settings {
        enabled: true,
        request_timeout: Duration::from_secs(5),
        diagnostics_wait: Duration::from_secs(2),
        ..Settings::default()
    }
}

fn mock_lsp(ws: &Path, launcher: &MockLauncher) -> Lsp {
    Lsp::new(
        ws.to_path_buf(),
        vec![detected("rust", &["rs"])],
        Box::new(launcher.clone()),
        settings(),
    )
}

fn nothing_hidden() -> Vec<PathBuf> {
    Vec::new()
}

fn at(pos: Position) -> impl Fn(&str) -> Value + Send + Sync {
    move |uri: &str| json!({"textDocument": {"uri": uri}, "position": pos, "context": {"includeDeclaration": true}})
}

/// (file name, 1-based line, 1-based column) of each location.
fn places(v: &Value, ws: &Path) -> Vec<(String, u32, u32)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|l| {
            let path = url::Url::parse(l["uri"].as_str().unwrap())
                .unwrap()
                .to_file_path()
                .unwrap();
            let text = std::fs::read_to_string(&path).unwrap();
            let pos: Position = serde_json::from_value(l["range"]["start"].clone()).unwrap();
            let (line, col) = from_lsp(&text, pos);
            (
                path.strip_prefix(ws).unwrap().display().to_string(),
                line,
                col,
            )
        })
        .collect()
}

#[tokio::test]
async fn one_server_answers_every_request_with_utf16_positions() {
    let (_d, ws) = workspace();
    let launcher = MockLauncher::default();
    let lsp = mock_lsp(&ws, &launcher);
    let main = Path::new("src/main.rs");
    // `add_one` on line 5 comes after a character that takes two UTF-16 units.
    let col = MAIN
        .lines()
        .nth(4)
        .unwrap()
        .chars()
        .position(|c| c == 'a')
        .unwrap() as u32
        + 1;
    let pos = to_lsp(MAIN, 5, col).unwrap();
    assert_eq!(
        pos.character, col,
        "one extra unit for the emoji, minus the 1-based column"
    );
    let defs = lsp
        .request(
            main,
            MAIN,
            &nothing_hidden,
            "textDocument/definition",
            &at(pos),
        )
        .await
        .unwrap();
    assert_eq!(places(&defs, &ws), [("src/main.rs".into(), 2, 4)]);
    let refs = lsp
        .request(
            main,
            MAIN,
            &nothing_hidden,
            "textDocument/references",
            &at(pos),
        )
        .await
        .unwrap();
    assert_eq!(
        places(&refs, &ws),
        [
            ("src/main.rs".into(), 2, 4),
            ("src/main.rs".into(), 5, col),
            ("src/util.rs".into(), 1, 15)
        ]
    );
    let hover = lsp
        .request(main, MAIN, &nothing_hidden, "textDocument/hover", &at(pos))
        .await
        .unwrap();
    let text = hover["contents"]["value"].as_str().unwrap();
    assert!(
        text.contains("fn add_one(x: i32)") && text.contains("Adds one."),
        "{text}"
    );
    let symbols = lsp
        .request(
            main,
            MAIN,
            &nothing_hidden,
            "textDocument/documentSymbol",
            &|uri: &str| json!({"textDocument": {"uri": uri}}),
        )
        .await
        .unwrap();
    let names: Vec<&str> = symbols
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["add_one", "main"]);
    let ws_symbols = lsp
        .workspace_request(
            "rust",
            &nothing_hidden,
            "workspace/symbol",
            json!({"query": "help"}),
        )
        .await
        .unwrap();
    assert_eq!(ws_symbols[0]["name"], "helper");
    assert_eq!(
        launcher.launches.load(Ordering::SeqCst),
        1,
        "one server for the run"
    );
    assert!(lsp.active("rust").await);

    // The server's own requests were answered.
    let log = lsp
        .request(main, MAIN, &nothing_hidden, "mock/log", &|_: &str| {
            Value::Null
        })
        .await
        .unwrap();
    let log = log.as_array().unwrap();
    let reply = |m: &str| log.iter().find(|e| e["reply_to"] == m).cloned().unwrap();
    assert_eq!(
        reply("workspace/configuration")["result"],
        json!([{"answer": 42}])
    );
    assert_eq!(
        reply("window/workDoneProgress/create")["result"],
        Value::Null
    );
    assert_eq!(reply("client/registerCapability")["error"], Value::Null);
    assert_eq!(
        log[0]["initialize"]["general"]["positionEncodings"],
        json!(["utf-16"])
    );
    // The document was opened once and never re-sent unchanged.
    let opens = log
        .iter()
        .filter(|e| e["notification"] == "textDocument/didOpen")
        .count();
    let changes = log
        .iter()
        .filter(|e| e["notification"] == "textDocument/didChange")
        .count();
    assert_eq!((opens, changes), (1, 0));
    lsp.shutdown().await;
    assert!(!lsp.active("rust").await);
}

#[tokio::test]
async fn edits_are_synced_and_their_diagnostics_collected() {
    let (_d, ws) = workspace();
    let launcher = MockLauncher::default();
    let lsp = mock_lsp(&ws, &launcher);
    let util = Path::new("src/util.rs");
    let wait = Duration::from_secs(2);
    // Nothing is started just for diagnostics unless asked.
    assert!(matches!(
        lsp.diagnostics(util, UTIL, &nothing_hidden, false, wait)
            .await,
        Ok(None)
    ));
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 0);
    let broken = "fn helper() { ERROR }\nfn b() { WARN }\n";
    let p = lsp
        .diagnostics(util, broken, &nothing_hidden, true, wait)
        .await
        .unwrap()
        .unwrap();
    let got: Vec<(u32, Option<u8>)> = p
        .items
        .iter()
        .map(|d| (d.range.start.line, d.severity))
        .collect();
    assert_eq!(got, [(0, Some(1)), (1, Some(2))]);
    let fixed = lsp
        .diagnostics(util, UTIL, &nothing_hidden, false, wait)
        .await
        .unwrap()
        .unwrap();
    assert!(fixed.items.is_empty(), "{:?}", fixed.items);
    assert!(fixed.seq > p.seq);
}

#[tokio::test]
async fn diagnostics_wait_for_a_check_in_progress() {
    let (_d, ws) = workspace();
    let launcher = MockLauncher {
        opts: MockOptions {
            slow_check: true,
            ..MockOptions::default()
        },
        ..MockLauncher::default()
    };
    let lsp = mock_lsp(&ws, &launcher);
    let util = Path::new("src/util.rs");
    let broken = "fn helper() { ERROR }\n";
    let p = lsp
        .diagnostics(util, broken, &nothing_hidden, true, Duration::from_secs(3))
        .await
        .unwrap()
        .unwrap();
    // The quick, empty publication inside the progress is not the answer.
    assert_eq!(p.items.len(), 1, "{:?}", p.items);
    // A wait shorter than the check returns what was there when it ran out.
    let p = lsp
        .diagnostics(
            util,
            UTIL,
            &nothing_hidden,
            false,
            Duration::from_millis(100),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(p.items.is_empty());
}

#[tokio::test]
async fn a_crashed_server_is_restarted_once_then_unavailable() {
    let (_d, ws) = workspace();
    std::fs::write(ws.join("src/c.rs"), "fn crash() {}\nfn crash_once() {}\n").unwrap();
    let text = std::fs::read_to_string(ws.join("src/c.rs")).unwrap();
    let launcher = MockLauncher::default();
    let lsp = mock_lsp(&ws, &launcher);
    let c = Path::new("src/c.rs");
    let hover = |line| at(Position { line, character: 4 });
    // Crashes in the first session only: the restart answers.
    let ok = lsp
        .request(c, &text, &nothing_hidden, "textDocument/hover", &hover(1))
        .await
        .unwrap();
    assert!(
        ok["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("crash_once")
    );
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 2);
    // Crashes every time: after its one restart it is given up on.
    let err = lsp
        .request(c, &text, &nothing_hidden, "textDocument/hover", &hover(0))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, LspError::Unavailable { language, .. } if language == "rust"),
        "{err:?}"
    );
    assert!(
        err.to_string()
            .contains("unavailable for the rest of this run")
    );
    let again = lsp
        .request(
            c,
            &text,
            &nothing_hidden,
            "textDocument/definition",
            &hover(1),
        )
        .await;
    assert!(matches!(again, Err(LspError::Unavailable { .. })));
    assert_eq!(
        launcher.launches.load(Ordering::SeqCst),
        2,
        "never more than two starts"
    );
    let events: Vec<String> = lsp.take_events().into_iter().map(|e| e.what).collect();
    assert_eq!(
        events,
        ["started", "crashed", "restarted", "crashed", "unavailable"]
    );
}

#[tokio::test]
async fn a_request_that_is_not_answered_times_out_and_the_server_stays() {
    let (_d, ws) = workspace();
    let launcher = MockLauncher {
        opts: MockOptions {
            hang_on: Some("textDocument/hover".into()),
            ..MockOptions::default()
        },
        ..MockLauncher::default()
    };
    let mut s = settings();
    s.request_timeout = Duration::from_millis(300);
    let lsp = Lsp::new(
        ws.clone(),
        vec![detected("rust", &["rs"])],
        Box::new(launcher.clone()),
        s,
    );
    let main = Path::new("src/main.rs");
    let pos = Position {
        line: 1,
        character: 4,
    };
    let err = lsp
        .request(main, MAIN, &nothing_hidden, "textDocument/hover", &at(pos))
        .await
        .unwrap_err();
    assert_eq!(err, LspError::Call(CallError::Timeout));
    assert!(
        lsp.request(
            main,
            MAIN,
            &nothing_hidden,
            "textDocument/definition",
            &at(pos)
        )
        .await
        .is_ok()
    );
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_newly_hidden_path_restarts_the_server_without_it() {
    let (_d, ws) = workspace();
    let launcher = MockLauncher::default();
    let lsp = mock_lsp(&ws, &launcher);
    let main = Path::new("src/main.rs");
    let hidden = std::sync::Mutex::new(vec![ws.join(".git")]);
    let current = || hidden.lock().unwrap().clone();
    let pos = Position {
        line: 1,
        character: 4,
    };
    for _ in 0..2 {
        lsp.request(main, MAIN, &current, "textDocument/definition", &at(pos))
            .await
            .unwrap();
    }
    hidden.lock().unwrap().push(ws.join("src/util.rs"));
    lsp.request(main, MAIN, &current, "textDocument/definition", &at(pos))
        .await
        .unwrap();
    let denied = launcher.denied.lock().unwrap().clone();
    assert_eq!(denied.len(), 2, "{denied:?}");
    assert!(denied[1].contains(&ws.join("src/util.rs")));
    let events: Vec<String> = lsp.take_events().into_iter().map(|e| e.what).collect();
    assert_eq!(events, ["started", "refreshed", "started"]);
}

/// The mock as a real process in the OS sandbox.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn a_sandboxed_server_cannot_read_hidden_files_and_one_that_keeps_crashing_is_given_up() {
    let (d, ws) = workspace();
    std::fs::write(
        ws.join("src/secret.rs"),
        "fn secret_fn() { add_one(8977066); }\n",
    )
    .unwrap();
    let launcher = SandboxLauncher {
        kind: declass_sandbox::detect().unwrap(),
        workspace: ws.clone(),
        scratch: d.path().join("scratch"),
    };
    let mut server = detected("rust", &["rs"]);
    server.program = PathBuf::from(env!("CARGO_BIN_EXE_declass-lsp-mock"));
    let lsp = Lsp::new(ws.clone(), vec![server], Box::new(launcher), settings());
    let main = Path::new("src/main.rs");
    let hidden = || vec![ws.join("src/secret.rs")];
    let pos = Position {
        line: 1,
        character: 4,
    };
    let refs = lsp
        .request(main, MAIN, &hidden, "textDocument/references", &at(pos))
        .await
        .unwrap();
    let files: Vec<String> = places(&refs, &ws).into_iter().map(|p| p.0).collect();
    assert!(files.contains(&"src/util.rs".to_owned()), "{files:?}");
    assert!(!files.contains(&"src/secret.rs".to_owned()), "{files:?}");
    let syms = lsp
        .workspace_request(
            "rust",
            &hidden,
            "workspace/symbol",
            json!({"query": "secret"}),
        )
        .await
        .unwrap();
    assert_eq!(syms, json!([]));
    // A crash (the process exits) is recovered from by one restart.
    std::fs::write(ws.join("src/c.rs"), "fn crash_once() {}\n").unwrap();
    let c = Path::new("src/c.rs");
    let err = lsp
        .request(
            c,
            "fn crash_once() {}\n",
            &hidden,
            "textDocument/hover",
            &at(Position {
                line: 0,
                character: 4,
            }),
        )
        .await
        .unwrap_err();
    // Every process is a first session, so it crashes twice: unavailable.
    assert!(matches!(err, LspError::Unavailable { .. }), "{err:?}");
    let events: Vec<String> = lsp.take_events().into_iter().map(|e| e.what).collect();
    assert_eq!(
        events,
        ["started", "crashed", "restarted", "crashed", "unavailable"]
    );
    lsp.shutdown().await;
}
