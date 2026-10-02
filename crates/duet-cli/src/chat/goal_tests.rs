// SPDX-License-Identifier: GPL-3.0-or-later
//! Exercise the production goal/session driver without sockets or model calls.

use super::*;
use crate::goals::{State, Store};
use bytes::Bytes;
use duet_agent::RunConfig;
use duet_boundary::audit::AuditLog;
use duet_boundary::engine::Engine;
use duet_boundary::policy::Policy;
use duet_boundary::{GatedFrontier, OutboundGate};
use duet_provider::client::{HttpReply, Transport};
use duet_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use tokio::sync::Notify;

struct Step {
    tool: &'static str,
    args: Value,
    release: Option<Arc<Notify>>,
}

impl Step {
    fn reply(message: &str) -> Self {
        Self {
            tool: "reply",
            args: json!({"message": message}),
            release: None,
        }
    }

    fn ask(question: &str) -> Self {
        Self {
            tool: "ask_operator",
            args: json!({"question": question}),
            release: None,
        }
    }

    fn finish() -> Self {
        Self {
            tool: "finish",
            args: json!({"summary": "The objective is complete."}),
            release: None,
        }
    }
}

#[derive(Clone, Default)]
struct Frontier {
    steps: Arc<Mutex<VecDeque<Step>>>,
    bodies: Arc<Mutex<Vec<String>>>,
    started: Arc<Notify>,
}

impl Transport for Frontier {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        let mut bodies = self.bodies.lock().unwrap();
        bodies.push(String::from_utf8(body).unwrap());
        let index = bodies.len();
        drop(bodies);
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra goal request");
        self.started.notify_one();
        Box::pin(async move {
            if let Some(release) = step.release {
                release.notified().await;
            }
            let delta = json!({"choices": [{"delta": {"tool_calls": [{
                "index": 0, "id": format!("goal-{index}"), "type": "function",
                "function": {"name": step.tool, "arguments": step.args.to_string()}
            }]}, "finish_reason": "tool_calls"}]});
            let usage =
                json!({"choices": [], "usage": {"prompt_tokens": 100, "completion_tokens": 10}});
            let bytes = Bytes::from(format!(
                "data: {delta}\n\ndata: {usage}\n\ndata: [DONE]\n\n"
            ));
            Ok(HttpReply {
                status: 200,
                headers: vec![],
                body: futures_util::stream::iter([Ok(bytes)]).boxed(),
            })
        })
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    cfg: duet_config::Config,
    run: RunConfig,
    manifest: RunManifest,
    engine: Arc<Engine>,
    frontier: Frontier,
    gated: GatedFrontier,
    git: duet_git::Git,
}

impl Fixture {
    fn new(objective: &str, steps: Vec<Step>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().canonicalize().unwrap().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&workspace)
                .status()
                .unwrap()
                .success()
        );
        let run_dir = workspace.join(".duet/runs/goal-test");
        let engine = Engine::open(
            &run_dir,
            Policy {
                detect_secrets: true,
                detect_pii: true,
                ..Policy::default()
            },
            None,
        )
        .unwrap();
        engine.prime(&workspace, &[], objective);
        let frontier = Frontier::default();
        frontier.steps.lock().unwrap().extend(steps);
        let mut provider =
            ProviderConfig::new("https://frontier.example/v1", "test-model", Role::Frontier);
        provider.backoff_scale = 0.0;
        let provider = ChatProvider::new(provider, Box::new(frontier.clone())).unwrap();
        let (filter, check) = engine.outbound();
        let gated =
            OutboundGate::new(AuditLog::open(&directory.path().join("audit.jsonl")).unwrap())
                .with_filter(filter)
                .with_check(check)
                .wrap(provider);
        let run = RunConfig {
            mode: "hybrid".into(),
            price: Box::new(|_| 0.125),
            ..RunConfig::new(workspace.clone(), run_dir, objective)
        };
        let manifest = RunManifest {
            run_id: "goal-test".into(),
            mode: Mode::Hybrid,
            objective: objective.into(),
            checks: Some(Vec::new()),
            frontier_url: "https://frontier.example/v1".into(),
            frontier_model: "test-model".into(),
            frontier_dialect: Some("chat".into()),
            local: None,
            session: true,
            images: Vec::new(),
            files: Vec::new(),
        };
        Self {
            cfg: duet_config::Config::load(&directory.path().join("missing-owner.toml"), None)
                .unwrap(),
            git: duet_git::Git::locate().unwrap(),
            _directory: directory,
            run,
            manifest,
            engine,
            frontier,
            gated,
        }
    }

    fn goal(&self, max_turns: u64) -> Store {
        let mut goals = Store::load(&self.run.run_dir).unwrap();
        goals.start(&self.manifest.objective, max_turns).unwrap();
        goals
    }

    fn session(&self, resume: bool, frontier_usd: f64) -> Session<'_> {
        Session::open(
            &self.run,
            &self.gated,
            self.engine.as_ref(),
            &self.git,
            Arc::new(AtomicBool::new(false)),
            SessionLimits {
                frontier_usd,
                working_time: Duration::from_secs(60),
            },
            resume,
        )
        .unwrap()
    }

    async fn conduct(
        &self,
        session: &mut Session<'_>,
        goals: &mut Store,
        inbox: Arc<Inbox>,
        resume: bool,
        tty: bool,
    ) -> (bool, Option<Mode>) {
        let io = Io {
            inbox,
            tty,
            working: Arc::new(AtomicBool::new(false)),
            leave: Arc::new(AtomicBool::new(false)),
            screen: Screen::Plain { tty: false },
            _clipboard: Arc::new(Mutex::new(crate::images::ClipboardImages::new(
                self.run.workspace.clone(),
                64,
            ))),
        };
        let engine = self.engine.clone();
        let shown: Arc<dyn Fn(&str) -> String + Send + Sync> =
            Arc::new(move |text| engine.detokenize(text));
        let files = Mutex::new(Vec::new());
        let ctx = TurnCtx {
            cfg: &self.cfg,
            mode: self.manifest.mode,
            skills: &self.run.skills,
            run_dir: &self.run.run_dir,
            io: &io,
            shown: &shown,
            ws: &self.run.workspace,
            git: &self.git,
            presenter: self.engine.as_ref(),
            files: &files,
        };
        let limit = goals
            .current()
            .map_or(crate::goals::DEFAULT_MAX_TURNS, |goal| goal.max_turns);
        tokio::time::timeout(
            Duration::from_secs(15),
            super::conduct(
                session,
                &self.manifest,
                &self.cfg,
                &ctx,
                goals,
                resume,
                limit,
            ),
        )
        .await
        .expect("goal driver did not stop")
        .unwrap()
    }

    fn requests(&self) -> usize {
        self.frontier.bodies.lock().unwrap().len()
    }
}

fn closed_inbox(lines: &[&str]) -> Arc<Inbox> {
    let inbox = Arc::new(Inbox::default());
    for line in lines {
        inbox.push((*line).into());
    }
    inbox.close();
    inbox
}

#[tokio::test]
async fn automatic_goal_turns_finish_and_each_continuation_crosses_the_privacy_boundary() {
    const EMAIL: &str = "marta.kowalczyk@corp-mail.net";
    const CARD: &str = "4539148803436467";
    let objective =
        format!("Review the report for {EMAIL}, card_number={CARD}; verify the final result.");
    let fixture = Fixture::new(
        &objective,
        vec![
            Step::reply("First change checked."),
            Step::reply("Second change checked."),
            Step::finish(),
        ],
    );
    let mut goals = fixture.goal(5);
    let mut session = fixture.session(false, 10.0);
    let (closed, switch) = fixture
        .conduct(&mut session, &mut goals, closed_inbox(&[]), false, false)
        .await;
    assert!(!closed && switch.is_none());
    assert_eq!(fixture.requests(), 3);
    assert_eq!(session.turns(), 3);
    assert_eq!(goals.current().unwrap().state, State::Completed);
    assert_eq!(goals.current().unwrap().turns, 3);
    for body in fixture.frontier.bodies.lock().unwrap().iter() {
        assert!(
            !body.contains(EMAIL) && !body.contains(CARD),
            "private objective crossed boundary"
        );
        assert!(
            body.contains('⟨'),
            "the hybrid boundary must mask the objective"
        );
    }
}

#[tokio::test]
async fn goal_limit_stops_automatic_turns_and_resume_retains_counts_spend_and_worked_time() {
    let fixture = Fixture::new(
        "Improve and verify the parser",
        vec![Step::reply("One"), Step::reply("Two")],
    );
    let mut goals = fixture.goal(2);
    let mut session = fixture.session(false, 0.25);
    fixture
        .conduct(&mut session, &mut goals, closed_inbox(&[]), false, false)
        .await;
    assert_eq!(fixture.requests(), 2);
    assert_eq!(goals.current().unwrap().state, State::Exhausted);
    let cost = session.stats().cost_usd;
    let usage = session.stats().usage;
    let worked = session.worked();
    assert_eq!(cost, 0.25);
    assert!(worked > Duration::ZERO);
    session.end(false);
    let mut reopened = fixture.session(true, 0.25);
    let mut restored = Store::load(&fixture.run.run_dir).unwrap();
    assert_eq!(reopened.turns(), 2);
    assert_eq!(reopened.stats().cost_usd, cost);
    assert_eq!(reopened.stats().usage, usage);
    assert!(reopened.worked().abs_diff(worked) < Duration::from_micros(1));
    assert_eq!(reopened.spent(), Some("session.frontier_usd"));
    assert_eq!(restored.current().unwrap().turns, 2);
    assert!(restored.resume().is_err());
    fixture
        .conduct(&mut reopened, &mut restored, closed_inbox(&[]), true, false)
        .await;
    assert_eq!(fixture.requests(), 2);
}

#[tokio::test]
async fn asking_waits_for_an_answer_and_queued_quit_wins_over_another_automatic_turn() {
    let fixture = Fixture::new(
        "Produce the requested report",
        vec![
            Step::ask("Which output format?"),
            Step::reply("The requested format is selected."),
        ],
    );
    let mut goals = fixture.goal(4);
    let mut session = fixture.session(false, 10.0);
    fixture
        .conduct(&mut session, &mut goals, closed_inbox(&[]), false, false)
        .await;
    assert_eq!(fixture.requests(), 1);
    assert_eq!(goals.current().unwrap().state, State::Waiting);
    let (closed, _) = fixture
        .conduct(
            &mut session,
            &mut goals,
            closed_inbox(&["Use CSV output", "/quit"]),
            true,
            false,
        )
        .await;
    assert!(!closed);
    assert_eq!(fixture.requests(), 2, "quit must prevent a third request");
    assert_eq!(goals.current().unwrap().state, State::Paused);
    assert!(fixture.frontier.bodies.lock().unwrap()[1].contains("Use CSV output"));
}

#[tokio::test]
async fn pausing_during_an_in_flight_response_prevents_any_automatic_followup() {
    let release = Arc::new(Notify::new());
    let mut step = Step::reply("Progress received before pausing.");
    step.release = Some(release.clone());
    let fixture = Fixture::new("Continue until the result is verified", vec![step]);
    let mut goals = fixture.goal(5);
    let mut session = fixture.session(false, 10.0);
    let inbox = Arc::new(Inbox::default());
    let feed = inbox.clone();
    let started = fixture.frontier.started.clone();
    let controller = tokio::spawn(async move {
        started.notified().await;
        feed.push("/goal pause".into());
        feed.push("/quit".into());
        // Hold the frontier until the real input loop has processed the stop.
        while !feed.lock().lines.is_empty() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        feed.close();
        release.notify_one();
    });
    fixture
        .conduct(&mut session, &mut goals, inbox, false, false)
        .await;
    controller.await.unwrap();
    assert_eq!(fixture.requests(), 1);
    assert_eq!(goals.current().unwrap().state, State::Paused);
    assert_eq!(goals.current().unwrap().turns, 1);
}

#[tokio::test]
async fn exit_controls_win_before_autonomous_work_and_normal_sessions_can_stop_and_close() {
    for (tty, lines, goal) in [
        (false, vec!["/quit"], true),
        (true, vec![], true),
        (false, vec!["/stop", "/close"], false),
    ] {
        let fixture = Fixture::new("Do useful work", vec![]);
        let mut goals = if goal {
            fixture.goal(5)
        } else {
            Store::load(&fixture.run.run_dir).unwrap()
        };
        let mut session = fixture.session(false, 10.0);
        let (closed, switch) = fixture
            .conduct(&mut session, &mut goals, closed_inbox(&lines), true, tty)
            .await;
        assert_eq!(closed, !goal);
        assert!(switch.is_none());
        assert_eq!(fixture.requests(), 0);
        if goal {
            assert_eq!(goals.current().unwrap().state, State::Paused);
            assert_eq!(goals.current().unwrap().turns, 0);
        } else {
            assert!(goals.current().is_none());
        }
    }
}

#[tokio::test]
async fn invalid_initial_skill_keeps_the_session_open_for_a_corrected_request() {
    let fixture = Fixture::new(
        "/skill missing-workflow",
        vec![Step::reply("Handled the corrected request.")],
    );
    let mut session = fixture.session(false, 5.0);
    let mut goals = Store::load(&fixture.run.run_dir).unwrap();
    let inbox = Arc::new(Inbox::default());
    inbox.push("Please review the parser".into());
    inbox.push("/quit".into());
    inbox.close();
    let result = fixture
        .conduct(&mut session, &mut goals, inbox, false, false)
        .await;
    assert_eq!(result, (false, None));
    let sent = fixture.frontier.bodies.lock().unwrap();
    assert_eq!(sent.len(), 1, "invalid workflow must not call the model");
    assert!(sent[0].contains("Please review the parser"));
    assert!(!sent[0].contains("missing-workflow"));
}

#[tokio::test]
async fn images_received_while_busy_keep_their_next_message_and_fifo_order() {
    let release = Arc::new(Notify::new());
    let mut first = Step::reply("First task complete.");
    first.release = Some(release.clone());
    let mut fixture = Fixture::new(
        "Initial task",
        vec![
            first,
            Step::reply("Red screen reviewed."),
            Step::reply("Blue screen reviewed."),
        ],
    );
    fixture
        .cfg
        .set_owner("frontier.vision", toml::Value::Boolean(true))
        .unwrap();
    fixture.run.images.frontier_vision = true;
    let red = fixture.run.workspace.join("red screen.png");
    let blue = fixture.run.workspace.join("blue screen.png");
    std::fs::write(&red, duet_provider::image::solid_png(2, 2, [255, 0, 0])).unwrap();
    std::fs::write(&blue, duet_provider::image::solid_png(2, 2, [0, 0, 255])).unwrap();
    let mut session = fixture.session(false, 10.0);
    let mut goals = Store::load(&fixture.run.run_dir).unwrap();
    let inbox = Arc::new(Inbox::default());
    let feed = inbox.clone();
    let started = fixture.frontier.started.clone();
    let controller = tokio::spawn(async move {
        started.notified().await;
        feed.push(format!(
            "/image --public {}",
            crate::attachments::quoted(&red)
        ));
        feed.push("Inspect the red screen".into());
        feed.push(format!(
            "/image --public {}",
            crate::attachments::quoted(&blue)
        ));
        feed.push("Inspect the blue screen".into());
        feed.push("/quit".into());
        while !feed.lock().lines.is_empty() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        feed.close();
        release.notify_one();
    });
    fixture
        .conduct(&mut session, &mut goals, inbox, false, false)
        .await;
    controller.await.unwrap();
    let bodies = fixture.frontier.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3);
    assert!(!bodies[0].contains("Inspect the red screen"));
    assert!(bodies[1].contains("Inspect the red screen"));
    assert!(!bodies[1].contains("Inspect the blue screen"));
    assert_eq!(
        bodies[1].matches("\"type\":\"image_url\"").count(),
        1,
        "only the first queued image belongs in the first queued request"
    );
    assert!(bodies[2].contains("Inspect the blue screen"));
    assert_eq!(
        bodies[2].matches("\"type\":\"image_url\"").count(),
        2,
        "second image joins the retained first image in the conversation"
    );
    assert!(session.attached().is_empty());
}

#[tokio::test]
async fn a_queued_image_that_disappears_does_not_send_its_dependent_message() {
    assert_failed_image_keeps_dependent_message(true).await;
}

#[tokio::test]
async fn an_invalid_image_received_while_busy_does_not_send_its_dependent_message() {
    assert_failed_image_keeps_dependent_message(false).await;
}

async fn assert_failed_image_keeps_dependent_message(disappears: bool) {
    let release = Arc::new(Notify::new());
    let mut first = Step::reply("First task complete.");
    first.release = Some(release.clone());
    let mut fixture = Fixture::new("Initial task", vec![first]);
    fixture
        .cfg
        .set_owner("frontier.vision", toml::Value::Boolean(true))
        .unwrap();
    fixture.run.images.frontier_vision = true;
    let image = fixture.run.workspace.join("temporary screen.png");
    if disappears {
        std::fs::write(&image, duet_provider::image::solid_png(2, 2, [255, 0, 0])).unwrap();
    } else {
        std::fs::write(&image, b"This is not an image").unwrap();
    }
    let mut session = fixture.session(false, 10.0);
    let mut goals = Store::load(&fixture.run.run_dir).unwrap();
    let inbox = Arc::new(Inbox::default());
    let feed = inbox.clone();
    let started = fixture.frontier.started.clone();
    let controller = tokio::spawn(async move {
        started.notified().await;
        feed.push(format!(
            "/image --public {}",
            crate::attachments::quoted(&image)
        ));
        feed.push("Inspect the vanished image".into());
        feed.push("/quit".into());
        while !feed.lock().lines.is_empty() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        if disappears {
            std::fs::remove_file(&image).unwrap();
        }
        feed.close();
        release.notify_one();
    });
    fixture
        .conduct(&mut session, &mut goals, inbox, false, false)
        .await;
    controller.await.unwrap();
    let bodies = fixture.frontier.bodies.lock().unwrap();
    assert_eq!(
        bodies.len(),
        1,
        "a failed image must not silently become a text-only request"
    );
    assert!(!bodies[0].contains("Inspect the vanished image"));
    assert!(session.attached().is_empty());
}

#[tokio::test]
async fn a_failed_image_group_never_leaks_valid_siblings_into_the_next_request() {
    let release = Arc::new(Notify::new());
    let mut first = Step::reply("First task complete.");
    first.release = Some(release.clone());
    let mut fixture = Fixture::new(
        "Initial task",
        vec![first, Step::reply("Independent image reviewed.")],
    );
    fixture
        .cfg
        .set_owner("frontier.vision", toml::Value::Boolean(true))
        .unwrap();
    fixture.run.images.frontier_vision = true;
    let ws = &fixture.run.workspace;
    let valid = ws.join("valid sibling.png");
    let invalid = ws.join("invalid sibling.png");
    let independent = ws.join("independent.png");
    let text = ws.join("sibling.md");
    std::fs::write(&valid, duet_provider::image::solid_png(2, 2, [255, 0, 0])).unwrap();
    std::fs::write(&invalid, b"not an image").unwrap();
    std::fs::write(
        &independent,
        duet_provider::image::solid_png(2, 2, [0, 0, 255]),
    )
    .unwrap();
    std::fs::write(&text, "FAILED_GROUP_TEXT_MARKER").unwrap();
    let mut session = fixture.session(false, 10.0);
    let mut goals = Store::load(&fixture.run.run_dir).unwrap();
    let inbox = Arc::new(Inbox::default());
    let feed = inbox.clone();
    let started = fixture.frontier.started.clone();
    let controller = tokio::spawn(async move {
        started.notified().await;
        feed.push(format!("/attach {}", crate::attachments::quoted(&text)));
        for image in [valid, invalid] {
            feed.push(format!(
                "/image --public {}",
                crate::attachments::quoted(&image)
            ));
        }
        feed.push("Inspect failed attachment group".into());
        feed.push(format!(
            "/image --public {}",
            crate::attachments::quoted(&independent)
        ));
        feed.push("Inspect independent attachment group".into());
        feed.push("/quit".into());
        while !feed.lock().lines.is_empty() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        feed.close();
        release.notify_one();
    });
    fixture
        .conduct(&mut session, &mut goals, inbox, false, false)
        .await;
    controller.await.unwrap();
    let bodies = fixture.frontier.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert!(bodies[1].contains("Inspect independent attachment group"));
    assert!(!bodies[1].contains("Inspect failed attachment group"));
    assert!(!bodies[1].contains("FAILED_GROUP_TEXT_MARKER"));
    assert_eq!(bodies[1].matches("\"type\":\"image_url\"").count(), 1);
    assert!(session.attached().is_empty());
}
