// SPDX-License-Identifier: GPL-3.0-or-later
//! Run the real tool loop against an in-process frontier, without sockets.

use bytes::Bytes;
use declass_agent::{RunConfig, Terminal};
use declass_boundary::OutboundGate;
use declass_boundary::audit::AuditLog;
use declass_boundary::view::PassThrough;
use declass_provider::client::{HttpReply, Transport};
use declass_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

const GUIDANCE: &str = "Every parser change must preserve the violet sentinel.";
const ORIGINAL: &str = "original nested file contents\n";
const UPDATED: &str = "updated nested file with violet sentinel\n";

#[derive(Clone)]
struct Frontier {
    workspace: PathBuf,
    first: &'static str,
    requests: Arc<AtomicUsize>,
}

impl Transport for Frontier {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        let request: Value = serde_json::from_slice(&body).unwrap();
        let index = self.requests.fetch_add(1, Ordering::SeqCst);
        let tool_results: Vec<&str> = request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "tool")
            .map(|message| message["content"].as_str().unwrap())
            .collect();
        let calls = match index {
            0 => {
                // Nested rules must be discovered for the actual target, not
                // recursively loaded into every project's initial prompt.
                assert!(!String::from_utf8_lossy(&body).contains(GUIDANCE));
                let first_args = match self.first {
                    "read_file" => json!({"path": "parser/input.txt"}),
                    "write_file" => {
                        json!({"path": "parser/input.txt", "content": "premature write\n"})
                    }
                    "edit_file" => json!({"path": "parser/input.txt", "edits": [
                        {"old": ORIGINAL, "new": "premature edit\n"}
                    ]}),
                    _ => unreachable!(),
                };
                vec![
                    (self.first, first_args),
                    (
                        "write_file",
                        json!({"path": "created.txt", "content": "premature\n"}),
                    ),
                    (
                        "edit_file",
                        json!({"path": "existing.txt", "edits": [
                            {"old": "before\n", "new": "premature\n"}
                        ]}),
                    ),
                    ("run_command", json!({"command": "touch command-was-run"})),
                    ("finish", json!({"summary": "Premature completion."})),
                ]
            }
            1 => {
                // Inspect the live filesystem before supplying the retry:
                // none of the first batch's mutations may have happened.
                assert_eq!(
                    std::fs::read_to_string(self.workspace.join("parser/input.txt")).unwrap(),
                    ORIGINAL
                );
                assert_eq!(
                    std::fs::read_to_string(self.workspace.join("existing.txt")).unwrap(),
                    "before\n"
                );
                assert!(!self.workspace.join("created.txt").exists());
                assert!(!self.workspace.join("command-was-run").exists());
                assert_eq!(tool_results.len(), 5);
                assert!(tool_results[0].contains(GUIDANCE));
                assert!(tool_results[0].contains("file operation was not performed"));
                assert!(!tool_results[0].contains(ORIGINAL));
                for result in &tool_results[1..] {
                    assert!(
                        result.contains("not run: new scoped instructions"),
                        "{result}"
                    );
                }
                vec![
                    ("read_file", json!({"path": "parser/input.txt"})),
                    (
                        "write_file",
                        json!({"path": "parser/input.txt", "content": UPDATED}),
                    ),
                    (
                        "write_file",
                        json!({"path": "created.txt", "content": "after guidance\n"}),
                    ),
                    (
                        "edit_file",
                        json!({"path": "existing.txt", "edits": [
                            {"old": "before\n", "new": "after guidance\n"}
                        ]}),
                    ),
                ]
            }
            2 => {
                assert_eq!(tool_results.len(), 9);
                let retried = &tool_results[5..];
                assert!(retried[0].contains(ORIGINAL.trim_end()));
                assert!(retried[1].contains("replaced parser/input.txt"));
                assert!(retried[2].contains("created created.txt"));
                assert!(retried[3].contains("edited existing.txt"));
                assert!(retried.iter().all(|result| !result.contains(GUIDANCE)));
                assert_eq!(
                    std::fs::read_to_string(self.workspace.join("parser/input.txt")).unwrap(),
                    UPDATED
                );
                assert_eq!(
                    std::fs::read_to_string(self.workspace.join("created.txt")).unwrap(),
                    "after guidance\n"
                );
                assert_eq!(
                    std::fs::read_to_string(self.workspace.join("existing.txt")).unwrap(),
                    "after guidance\n"
                );
                vec![("finish", json!({"summary": "Applied the scoped guidance."}))]
            }
            _ => panic!("unexpected extra scoped-instruction request"),
        };
        let tool_calls: Vec<_> = calls
            .into_iter()
            .enumerate()
            .map(|(i, (name, args))| {
                json!({"index": i, "id": format!("scoped-{index}-{i}"), "type": "function",
                "function": {"name": name, "arguments": args.to_string()}})
            })
            .collect();
        let delta = json!({"choices": [{"delta": {"tool_calls": tool_calls}, "finish_reason": "tool_calls"}]});
        let usage =
            json!({"choices": [], "usage": {"prompt_tokens": 100, "completion_tokens": 10}});
        let bytes = Bytes::from(format!(
            "data: {delta}\n\ndata: {usage}\n\ndata: [DONE]\n\n"
        ));
        Box::pin(async move {
            Ok(HttpReply {
                status: 200,
                headers: vec![],
                body: futures_util::stream::iter([Ok(bytes)]).boxed(),
            })
        })
    }
}

async fn deferred_operation(first: &'static str) {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().canonicalize().unwrap().join("workspace");
    std::fs::create_dir_all(workspace.join("parser")).unwrap();
    std::fs::write(workspace.join("parser/AGENTS.md"), GUIDANCE).unwrap();
    std::fs::write(workspace.join("parser/input.txt"), ORIGINAL).unwrap();
    std::fs::write(workspace.join("existing.txt"), "before\n").unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&workspace)
            .status()
            .unwrap()
            .success()
    );
    let frontier = Frontier {
        workspace: workspace.clone(),
        first,
        requests: Arc::new(AtomicUsize::new(0)),
    };
    let mut provider =
        ProviderConfig::new("https://frontier.example/v1", "test-model", Role::Frontier);
    provider.backoff_scale = 0.0;
    let provider = ChatProvider::new(provider, Box::new(frontier.clone())).unwrap();
    let gated = OutboundGate::new(AuditLog::open(&directory.path().join("audit.jsonl")).unwrap())
        .wrap(provider);
    let cfg = RunConfig::new(
        &workspace,
        workspace.join(".declass/runs/scoped-test"),
        "Update the parser and supporting files.",
    );
    let (terminal, stats) = tokio::time::timeout(
        Duration::from_secs(15),
        declass_agent::run(
            &cfg,
            &gated,
            &PassThrough { max_bytes: 60_000 },
            &declass_git::Git::locate().unwrap(),
            false,
            &Arc::new(AtomicBool::new(false)),
        ),
    )
    .await
    .expect("scoped instruction run timed out");
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    assert_eq!(frontier.requests.load(Ordering::SeqCst), 3);
    assert_eq!(stats.turns, 3);
    assert!(!workspace.join("command-was-run").exists());
}

#[tokio::test]
async fn first_nested_read_delivers_guidance_before_read_or_batch_mutations() {
    deferred_operation("read_file").await;
}

#[tokio::test]
async fn first_nested_write_delivers_guidance_before_any_batch_mutation() {
    deferred_operation("write_file").await;
}

#[tokio::test]
async fn first_nested_edit_delivers_guidance_before_any_batch_mutation() {
    deferred_operation("edit_file").await;
}
