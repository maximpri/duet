// SPDX-License-Identifier: GPL-3.0-or-later
//! Scripted provider requests prove the plan lifecycle and privacy boundary without a model service.
use bytes::Bytes;
use declass_agent::plans::{Store, VerificationKind, VerificationStatus};
use declass_agent::{RunConfig, Session, SessionLimits, TurnEnd};
use declass_boundary::audit::AuditLog;
use declass_boundary::{OutboundGate, engine::Engine, policy::Policy};
use declass_provider::client::{HttpReply, Transport};
use declass_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::{StreamExt, future::BoxFuture};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, atomic::AtomicBool};
use std::time::Duration;

#[derive(Clone, Default)]
struct Frontier {
    calls: Arc<Mutex<VecDeque<(&'static str, Value)>>>,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Frontier {
    fn script(&self, calls: impl IntoIterator<Item = (&'static str, Value)>) {
        self.calls.lock().unwrap().extend(calls);
    }
}

impl Transport for Frontier {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        let mut requests = self.requests.lock().unwrap();
        requests.push(serde_json::from_slice(&body).unwrap());
        let id = format!("call{}", requests.len());
        let (name, args) = self
            .calls
            .lock()
            .unwrap()
            .pop_front()
            .expect("script exhausted");
        let delta = json!({"tool_calls": [{"index": 0, "id": id, "type": "function",
            "function": {"name": name, "arguments": args.to_string()}}]});
        let chunks = [
            format!("data: {}\n\n", json!({"choices": [{"delta": delta}]})),
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n".into(),
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":10}}\n\n"
                .into(),
            "data: [DONE]\n\n".into(),
        ];
        Box::pin(async move {
            Ok(HttpReply {
                status: 200,
                headers: Vec::new(),
                body: futures_util::stream::iter(chunks.into_iter().map(|s| Ok(Bytes::from(s))))
                    .boxed(),
            })
        })
    }
}

#[tokio::test]
async fn approved_plan_executes_checks_and_keeps_private_editor_text_off_outbound_requests() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap().join("workspace");
    let run = ws.join(".declass/runs/structured-plan");
    std::fs::create_dir_all(&run).unwrap();
    let engine = Engine::open(
        &run,
        Policy {
            detect_secrets: true,
            ..Policy::default()
        },
        None,
    )
    .unwrap();
    let secret = "sk-proj-PlanEditorPrivateCanary_123456789012345678901234567890";
    engine.prime(&ws, &[], secret);
    let frontier = Frontier::default();
    let mut provider = ProviderConfig::new("https://frontier.example/v1", "test", Role::Frontier);
    provider.backoff_scale = 0.0;
    let (filter, check) = engine.outbound();
    let gated = OutboundGate::new(AuditLog::open(&dir.path().join("audit.jsonl")).unwrap())
        .with_filter(filter)
        .with_check(check)
        .wrap(ChatProvider::new(provider, Box::new(frontier.clone())).unwrap());
    let mut cfg = RunConfig::new(ws.clone(), run.clone(), "Plan a repair");
    cfg.sandbox = declass_sandbox::detect().unwrap();
    cfg.checks = vec!["test -f implemented.txt && printf checked > original-check-ran".into()];
    let git = declass_git::Git::locate().unwrap();
    let mut session = Session::open(
        &cfg,
        &gated,
        engine.as_ref(),
        &git,
        Arc::new(AtomicBool::new(false)),
        SessionLimits {
            frontier_usd: 100.0,
            working_time: Duration::from_secs(90),
        },
        false,
    )
    .unwrap();
    session.set_planning(true).unwrap();
    frontier.script([
        ("propose_plan",json!({"draft":{
            "title":"Repair","objective":"Implement the requested change","scope":[],"non_goals":[],"assumptions":[],
            "steps":[{"id":"","title":"Implement","description":"Create implementation","paths":["implemented.txt"],"checks":[{"id":"","command":"test -f implemented.txt","description":"Implementation exists"}]}]
        }})),
        ("run_command",json!({"command":"printf bypass > forbidden"})),
        ("finish",json!({"summary":"forged completion"})),
        ("ask_operator",json!({"question":"Which implementation?","options":{"choices":[{"id":"small","label":"Small"},{"id":"large","label":"Large"}],"recommended":"small"}})),
    ]);
    let end = session.turn("Inspect and propose first").await;
    let TurnEnd::Asked {
        question,
        options: Some(options),
    } = end
    else {
        panic!("expected structured clarification")
    };
    assert_eq!(question, "Which implementation?");
    assert_eq!(options.recommended.as_deref(), Some("small"));
    assert!(!ws.join("forbidden").exists());
    assert!(!ws.join("original-check-ran").exists());
    let mut plans = Store::load(&run).unwrap();
    let revision = plans.current().unwrap().clone();
    assert!(
        plans.approve(&revision.id, &revision.digest).is_err(),
        "unanswered question must block approval"
    );
    plans
        .answer_question(&revision.id, &options.id, "Small")
        .unwrap();
    let mut edited = revision.draft;
    edited
        .assumptions
        .push(format!("Private owner note: {secret}"));
    let approved = plans.revise(Some(&revision.id), edited).unwrap();
    let step = approved.draft.steps[0].id.clone();
    plans.approve(&approved.id, &approved.digest).unwrap();
    session.set_planning(false).unwrap();
    assert_eq!(
        frontier.requests.lock().unwrap().len(),
        4,
        "mode exit must not call provider"
    );
    drop(session);
    let mut session = Session::open(
        &cfg,
        &gated,
        engine.as_ref(),
        &git,
        Arc::new(AtomicBool::new(false)),
        SessionLimits {
            frontier_usd: 100.0,
            working_time: Duration::from_secs(90),
        },
        true,
    )
    .unwrap();
    assert_eq!(
        frontier.requests.lock().unwrap().len(),
        4,
        "resume must not execute a plan"
    );
    plans.implement(&approved.id, &approved.digest, 5).unwrap();
    plans.pause("Unrelated ordinary work").unwrap();
    let previous_questions = plans.snapshot().questions.len();
    frontier.script([(
        "ask_operator",
        json!({"question":"Unrelated clarification?"}),
    )]);
    assert!(matches!(
        session
            .turn("Discuss unrelated work while the plan is paused")
            .await,
        TurnEnd::Asked { .. }
    ));
    plans.refresh().unwrap();
    assert_eq!(
        plans.snapshot().questions.len(),
        previous_questions,
        "ordinary questions must not attach to or resume a paused plan"
    );
    assert_eq!(
        plans.snapshot().execution.as_ref().unwrap().status,
        declass_agent::plans::ExecutionStatus::Paused
    );
    plans.resume(&approved.id, &approved.digest).unwrap();
    plans.begin_turn().unwrap();
    frontier.script([
        ("read_plan",json!({})),
        ("write_file",json!({"path":"implemented.txt","content":"implemented\n"})),
        ("update_plan_step",json!({"revision":approved.id,"step_id":step,"status":"done_unverified","note":"implemented"})),
        ("finish",json!({"summary":"Implemented and checked"})),
    ]);
    let end = session.turn("Execute the approved plan").await;
    assert!(matches!(end, TurnEnd::Completed { .. }), "{end:?}");
    plans.refresh().unwrap();
    plans.finish_turn(&end).unwrap();
    let execution = plans.snapshot().execution.as_ref().unwrap();
    assert!(execution.evidence.iter().any(|v|v.kind==VerificationKind::Proposed && v.status==VerificationStatus::Passed));
    assert!(
        execution
            .evidence
            .iter()
            .any(|v| v.kind == VerificationKind::Configured
                && v.status == VerificationStatus::Passed)
    );
    assert!(ws.join("original-check-ran").exists());
    assert!(plans.render_markdown().contains(secret));
    for observed in frontier.requests.lock().unwrap().iter() {
        assert!(
            !observed.to_string().contains(secret),
            "private editor note leaked into outbound request"
        );
    }
}
