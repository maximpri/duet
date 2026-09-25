// SPDX-License-Identifier: GPL-3.0-or-later
//! Images through the frontier loop, with a scripted frontier and a scripted
//! local model (no model server): `read_file` on images and the operator's
//! attachments are routed by the rules in hybrid and pass-through mode. A
//! sensitive-path image never reaches the frontier (its bytes are in no
//! request, whole or in part), its local description is filtered, and a
//! refused image stops the run before anything is sent. Images the frontier
//! may see are stored by digest in the run directory, the transcript holds
//! digests only, and a resumed run or session sends them again unchanged.
//! Audit records hold digests, never image data.

use bytes::Bytes;
use duet_agent::images::{Attachment, ImageConfig};
use duet_agent::session::{SessionLimits, TurnEnd};
use duet_agent::{RunConfig, Session, Terminal};
use duet_boundary::OutboundGate;
use duet_boundary::audit::{AuditEvent, AuditLog, Line, read};
use duet_boundary::engine::Engine;
use duet_boundary::images::ToFrontier;
use duet_boundary::local::LocalReader;
use duet_boundary::policy::Policy;
use duet_boundary::view::{PassThrough, Presenter};
use duet_provider::client::{HttpReply, Transport};
use duet_provider::image::{Image, pattern_png, prepare};
use duet_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SECRET: &str = "sk_live_Rt5Yh8Kp2Nw6Qz9Lm3Xv";
const EMAIL: &str = "joris.bakkeveen@holt-mail.net";
const NAME: &str = "Joris Bakkeveen";
const CARD: &str = "4539 1488 0343 6467";

/// What the scripted frontier answers one request with.
enum Step {
    Call(&'static str, Value),
    /// A message without tool calls.
    Text(&'static str),
    /// An HTTP error that is not retried.
    Fail(u16),
}

#[derive(Clone, Default)]
struct Frontier {
    steps: Arc<Mutex<VecDeque<Step>>>,
    bodies: Arc<Mutex<Vec<String>>>,
}

impl Frontier {
    fn new(steps: Vec<Step>) -> Self {
        let f = Self::default();
        f.steps.lock().unwrap().extend(steps);
        f
    }

    fn bodies(&self) -> Vec<String> {
        self.bodies.lock().unwrap().clone()
    }
}

impl Transport for Frontier {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        self.bodies
            .lock()
            .unwrap()
            .push(String::from_utf8(body).unwrap());
        let n = self.bodies.lock().unwrap().len();
        let step = self.steps.lock().unwrap().pop_front().expect("unscripted");
        let (status, chunks): (u16, Vec<String>) = match step {
            Step::Call(name, args) => {
                let call = json!({"choices": [{"delta": {"tool_calls": [{"index": 0,
                    "id": format!("c{n}"), "type": "function",
                    "function": {"name": name, "arguments": args.to_string()}}]}}]});
                (
                    200,
                    vec![
                        format!("data: {call}\n\n"),
                        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n"
                            .into(),
                    ],
                )
            }
            Step::Text(t) => (
                200,
                vec![format!(
                    "data: {}\n\n",
                    json!({"choices": [{"delta": {"content": t}, "finish_reason": "stop"}]})
                )],
            ),
            Step::Fail(status) => (status, vec![r#"{"error":{"message":"bad"}}"#.into()]),
        };
        let mut chunks = chunks;
        if status == 200 {
            chunks.push(
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1000,\"completion_tokens\":20}}\n\n"
                    .into(),
            );
            chunks.push("data: [DONE]\n\n".into());
        }
        Box::pin(async move {
            Ok(HttpReply {
                status,
                headers: vec![],
                body: futures_util::stream::iter(chunks.into_iter().map(|c| Ok(Bytes::from(c))))
                    .boxed(),
            })
        })
    }
}

/// A workspace with a sensitive image (`data/scan.png`), a public one
/// (`docs/ui.png`), sensitive text, and an image outside it.
struct Ws {
    _dir: tempfile::TempDir,
    root: PathBuf,
    ws: PathBuf,
}

impl Ws {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let ws = root.join("ws");
        for d in ["data", "docs", "src"] {
            std::fs::create_dir_all(ws.join(d)).unwrap();
        }
        std::fs::create_dir_all(root.join("outside")).unwrap();
        std::fs::write(ws.join("data/scan.png"), pattern_png(96, 64, 1)).unwrap();
        std::fs::write(ws.join("docs/ui.png"), pattern_png(80, 40, 2)).unwrap();
        std::fs::write(root.join("outside/photo.png"), pattern_png(50, 50, 3)).unwrap();
        std::fs::write(ws.join(".env"), format!("STRIPE_KEY={SECRET}\n")).unwrap();
        std::fs::write(
            ws.join("data/customers.csv"),
            format!("id,name,email,card\n1,{NAME},{EMAIL},{CARD}\n"),
        )
        .unwrap();
        std::fs::write(ws.join("src/lib.rs"), "pub fn render() {}\n").unwrap();
        Self {
            _dir: dir,
            root,
            ws,
        }
    }

    fn run_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    fn log(&self) -> PathBuf {
        self.root.join("audit.jsonl")
    }

    /// The image `rel` as a model would get it (prepared as the run does).
    fn prepared(&self, rel: &str) -> Image {
        prepare(&std::fs::read(self.path(rel)).unwrap(), 1568).unwrap()
    }

    fn path(&self, rel: &str) -> PathBuf {
        match rel.strip_prefix("outside/") {
            Some(name) => self.root.join("outside").join(name),
            None => self.ws.join(rel),
        }
    }
}

fn policy(to_frontier: ToFrontier, local_vision: bool) -> Policy {
    Policy {
        sensitive_globs: vec![".env*".into(), "data/**".into()],
        command_output_sensitive: true,
        detect_secrets: true,
        detect_pii: true,
        detect_entropy: true,
        bulky_tokens: 2000,
        bulky_file_tokens: 12000,
        local_vision,
        images_to_frontier: to_frontier,
        ..Policy::default()
    }
}

/// The primed engine over the workspace, with a scripted local model.
fn engine(w: &Ws, policy: Policy, local_replies: Vec<String>) -> Arc<Engine> {
    let (local, _) = duet_boundary::testing::scripted_local(local_replies);
    engine_with(w, policy, Some(local))
}

fn engine_with(w: &Ws, policy: Policy, local: Option<LocalReader>) -> Arc<Engine> {
    let e = Engine::open(&w.run_dir(), policy, local).unwrap();
    e.prime(
        &w.ws,
        &[
            ".env".to_owned(),
            "data/customers.csv".to_owned(),
            "data/scan.png".to_owned(),
            "docs/ui.png".to_owned(),
            "src/lib.rs".to_owned(),
        ],
        "Fix the layout.",
    );
    e
}

fn config(w: &Ws, images: ImageConfig) -> RunConfig {
    RunConfig {
        workspace: w.ws.clone(),
        run_dir: w.run_dir(),
        objective: "Fix the layout.".into(),
        mode: "hybrid".into(),
        checks: vec![],
        sandbox: duet_sandbox::detect().unwrap_or(duet_sandbox::SandboxKind::Seatbelt),
        network: false,
        command_timeout: Duration::from_secs(30),
        wall_clock: Duration::from_secs(60),
        frontier_usd: 10.0,
        max_finish_attempts: 2,
        context_window: 200_000,
        mask_at: 0.7,
        max_output_tokens: 1000,
        reasoning_effort: None,
        price: Box::new(|u| u.input as f64 / 1e6),
        oversight: duet_agent::Oversight::default(),
        web: None,
        git_author: None,
        mcp: None,
        lsp: None,
        images,
        subagents: None,
    }
}

fn images(frontier_vision: bool, attached: Vec<Attachment>) -> ImageConfig {
    ImageConfig {
        frontier_vision,
        max_side: 1568,
        attached,
    }
}

fn gated(
    w: &Ws,
    frontier: &Frontier,
    engine: Option<&Arc<Engine>>,
) -> duet_boundary::GatedFrontier {
    let mut pc = ProviderConfig::new("https://frontier.example/v1", "m", Role::Frontier);
    pc.backoff_scale = 0.0;
    let provider = ChatProvider::new(pc, Box::new(frontier.clone())).unwrap();
    let mut gate = OutboundGate::new(AuditLog::open(&w.log()).unwrap());
    if let Some(e) = engine {
        let (filter, check) = e.outbound();
        gate = gate.with_filter(filter).with_check(check);
    }
    gate.wrap(provider)
}

async fn run(
    w: &Ws,
    cfg: &RunConfig,
    presenter: &dyn Presenter,
    engine: Option<&Arc<Engine>>,
    frontier: &Frontier,
    resume: bool,
) -> Terminal {
    let gated = gated(w, frontier, engine);
    let git = duet_git::Git::locate().unwrap();
    duet_agent::run(
        cfg,
        &gated,
        presenter,
        &git,
        resume,
        &Arc::new(AtomicBool::new(false)),
    )
    .await
    .0
}

fn image_events(log: &Path) -> Vec<AuditEvent> {
    read(log)
        .unwrap()
        .into_iter()
        .filter_map(|l| match l {
            Line::Event(e) if matches!(e.event, AuditEvent::Image { .. }) => Some(e.event),
            _ => None,
        })
        .collect()
}

/// Whether any part of `image`'s data is in `text`: its base64 whole, or any
/// 48-character window of it (a partial copy), or the base64 of the file as
/// stored in the workspace.
fn carries(text: &str, image: &Image, file: &[u8]) -> bool {
    let encoded = [
        image.base64(),
        Image::from_encoded(file.to_vec()).unwrap().base64(),
    ];
    encoded.iter().any(|b64| {
        text.contains(b64.as_str())
            || (0..b64.len().saturating_sub(48))
                .step_by(97)
                .any(|at| text.contains(&b64[at..at + 48]))
    })
}

/// Planted values in `text`: as written, JSON-escaped, or without spaces.
fn leaked(text: &str) -> Vec<&'static str> {
    [SECRET, EMAIL, NAME, CARD, "Bakkeveen", "4539148803436467"]
        .into_iter()
        .filter(|v| {
            let escaped = serde_json::to_string(v).unwrap();
            text.contains(v) || text.contains(escaped.trim_matches('"'))
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sensitive_image_never_reaches_the_frontier_and_its_description_is_filtered() {
    let w = Ws::new();
    // The most permissive hybrid settings: workspace images may go to the
    // frontier, which reads images; the local model reads them too and
    // "describes" the sensitive one by copying the values it shows.
    let describe = json!({"summary": format!(
        "A scanned billing form for {NAME} ({EMAIL}) with card {CARD}; a sticky note shows {SECRET}."),
        "facts": [format!("Card holder: {NAME}"), "Two columns, a signature box."]})
    .to_string();
    let e = engine(&w, policy(ToFrontier::Public, true), vec![describe]);
    let cfg = config(&w, images(true, vec![]));
    let frontier = Frontier::new(vec![
        Step::Call("read_file", json!({"path": "data/scan.png"})),
        Step::Call("read_file", json!({"path": "docs/ui.png"})),
        Step::Call("finish", json!({"summary": "looked"})),
    ]);
    let terminal = run(&w, &cfg, e.as_ref(), Some(&e), &frontier, false).await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );

    let bodies = frontier.bodies();
    let all = bodies.concat();
    let scan = w.prepared("data/scan.png");
    let scan_file = std::fs::read(w.path("data/scan.png")).unwrap();
    assert!(
        !carries(&all, &scan, &scan_file),
        "the sensitive image reached the frontier"
    );
    assert!(
        leaked(&all).is_empty(),
        "{:?} reached the frontier",
        leaked(&all)
    );
    // The frontier got the cleaned description and a handle, and the public
    // image itself (images.to_frontier = "public", frontier.vision on).
    let last: Value = serde_json::from_str(bodies.last().unwrap()).unwrap();
    let messages = last["messages"].as_array().unwrap();
    let described = messages
        .iter()
        .find(|m| m["role"] == "tool" && m["content"].as_str().unwrap().contains("data/scan.png"))
        .unwrap();
    let text = described["content"].as_str().unwrap();
    assert!(text.contains("Two columns, a signature box"), "{text}");
    assert!(text.contains("ask_local(handle="), "{text}");
    let ui = w.prepared("docs/ui.png");
    assert!(
        all.contains(&ui.data_url()),
        "the public image was not sent"
    );
    // (The same check finds an image that was sent.)
    assert!(carries(
        &all,
        &ui,
        &std::fs::read(w.path("docs/ui.png")).unwrap()
    ));
    let image_message = messages
        .iter()
        .find(|m| m["role"] == "user" && m["content"].is_array())
        .unwrap();
    assert_eq!(
        image_message["content"][1]["image_url"]["url"],
        ui.data_url()
    );

    // The public image is stored by digest; the sensitive one only as a handle.
    assert!(
        w.run_dir()
            .join(format!("images/{}.png", ui.sha256))
            .exists()
    );
    assert!(
        !w.run_dir()
            .join(format!("images/{}.png", scan.sha256))
            .exists()
    );
    let transcript = std::fs::read_to_string(w.run_dir().join("transcript.jsonl")).unwrap();
    assert!(transcript.contains(&ui.sha256) && !transcript.contains(&ui.base64()));

    // The audit log: one event per image, digests and no image data.
    let events = image_events(&w.log());
    assert_eq!(
        events,
        vec![
            AuditEvent::Image {
                origin: "workspace:data/scan.png".into(),
                bytes: scan.bytes,
                sha256: scan.sha256.clone(),
                destination: "local".into(),
                decision: "sensitive_path".into(),
                operator_public: false,
            },
            AuditEvent::Image {
                origin: "workspace:docs/ui.png".into(),
                bytes: ui.bytes,
                sha256: ui.sha256.clone(),
                destination: "frontier".into(),
                decision: "images.to_frontier".into(),
                operator_public: false,
            },
        ]
    );
    let log = std::fs::read_to_string(w.log()).unwrap();
    assert!(!carries(&log, &scan, &scan_file) && !log.contains(&ui.base64()));
    assert!(log.contains(&format!("[image sha256:{}", ui.sha256)));
    assert!(
        leaked(&log).is_empty(),
        "{:?} in the audit log",
        leaked(&log)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_vision_model_images_are_refused_with_the_reason() {
    let w = Ws::new();
    // Hybrid defaults: images.to_frontier = "never", no local vision.
    let e = engine(&w, policy(ToFrontier::Never, false), vec![]);
    let cfg = config(&w, images(true, vec![]));
    let frontier = Frontier::new(vec![
        Step::Call("read_file", json!({"path": "docs/ui.png"})),
        Step::Call("read_file", json!({"path": "data/scan.png"})),
        Step::Call("finish", json!({"summary": "could not look"})),
    ]);
    let terminal = run(&w, &cfg, e.as_ref(), Some(&e), &frontier, false).await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    let all = frontier.bodies().concat();
    assert!(!all.contains("data:image/"), "an image was sent");
    assert!(
        all.contains("docs/ui.png is an image and was not shown"),
        "{all}"
    );
    assert!(all.contains("local.vision is false"), "{all}");
    let decisions: Vec<(String, String)> = image_events(&w.log())
        .into_iter()
        .map(|e| match e {
            AuditEvent::Image {
                destination,
                decision,
                ..
            } => (destination, decision),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        decisions,
        vec![
            ("none".to_owned(), "no_local_vision".to_owned()),
            ("none".to_owned(), "no_local_vision".to_owned())
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pass_through_sends_images_only_to_a_frontier_that_takes_them() {
    let w = Ws::new();
    let presenter = PassThrough { max_bytes: 60_000 };
    for vision in [true, false] {
        let w2 = Ws::new();
        let cfg = config(&w2, images(vision, vec![]));
        let frontier = Frontier::new(vec![
            Step::Call("read_file", json!({"path": "data/scan.png"})),
            Step::Call("finish", json!({"summary": "done"})),
        ]);
        let terminal = run(&w2, &cfg, &presenter, None, &frontier, false).await;
        assert!(
            matches!(terminal, Terminal::Completed { .. }),
            "{terminal:?}"
        );
        let all = frontier.bodies().concat();
        let scan = w2.prepared("data/scan.png");
        assert_eq!(all.contains(&scan.data_url()), vision);
        if !vision {
            assert!(all.contains("frontier.vision is false"), "{all}");
        }
        let decision = match &image_events(&w2.log())[0] {
            AuditEvent::Image { decision, .. } => decision.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            decision,
            if vision {
                "passthrough"
            } else {
                "no_frontier_vision"
            }
        );
    }
    drop(w);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_attachment_stops_the_run_before_anything_is_sent() {
    let w = Ws::new();
    for (attachment, local_vision, why) in [
        // Not public and no local vision: nobody may see it.
        (
            Attachment {
                path: w.path("outside/photo.png"),
                public: false,
            },
            false,
            "local.vision is false",
        ),
        // Marked public, but on a sensitive path.
        (
            Attachment {
                path: w.path("data/scan.png"),
                public: true,
            },
            true,
            "sensitive path",
        ),
        // Not an image.
        (
            Attachment {
                path: w.path("src/lib.rs"),
                public: true,
            },
            true,
            "cannot be read as an image",
        ),
    ] {
        let _ = std::fs::remove_dir_all(w.run_dir());
        let e = engine(&w, policy(ToFrontier::Never, local_vision), vec![]);
        let cfg = config(&w, images(true, vec![attachment]));
        let frontier = Frontier::new(vec![]);
        let terminal = run(&w, &cfg, e.as_ref(), Some(&e), &frontier, false).await;
        let Terminal::Failed { reason } = terminal else {
            panic!("{terminal:?}")
        };
        assert!(reason.contains(why), "{reason}");
        assert!(frontier.bodies().is_empty(), "a request was sent");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn attachments_are_sent_or_described_by_the_rules() {
    let w = Ws::new();
    let describe = json!({"summary": format!("A photo of a whiteboard; it names {EMAIL}."),
        "facts": ["Three boxes joined by arrows."]})
    .to_string();
    let e = engine(&w, policy(ToFrontier::Never, true), vec![describe]);
    let attached = vec![
        // The operator's decision: this one may go to the frontier.
        Attachment {
            path: w.path("outside/photo.png"),
            public: true,
        },
        // A workspace file on a sensitive path: described locally.
        Attachment {
            path: w.path("data/scan.png"),
            public: false,
        },
    ];
    let cfg = config(&w, images(true, attached));
    let frontier = Frontier::new(vec![Step::Call("finish", json!({"summary": "seen"}))]);
    let terminal = run(&w, &cfg, e.as_ref(), Some(&e), &frontier, false).await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    let first: Value = serde_json::from_str(&frontier.bodies()[0]).unwrap();
    let content = &first["messages"][1]["content"];
    let photo = w.prepared("outside/photo.png");
    assert_eq!(content[0]["image_url"]["url"], photo.data_url());
    let text = content[1]["text"].as_str().unwrap();
    assert!(text.starts_with("Fix the layout."), "{text}");
    assert!(text.contains("[image attached by the operator: photo.png (image, 50x50 png"));
    assert!(text.contains("Three boxes joined by arrows"), "{text}");
    assert!(!text.contains(EMAIL), "{text}");
    let all = frontier.bodies().concat();
    let scan = w.prepared("data/scan.png");
    let scan_file = std::fs::read(w.path("data/scan.png")).unwrap();
    assert!(!carries(&all, &scan, &scan_file));
    let events = image_events(&w.log());
    assert!(
        matches!(&events[0], AuditEvent::Image { origin, destination, decision, operator_public: true, .. }
        if origin == "attached:photo.png" && destination == "frontier" && decision == "operator_public")
    );
    assert!(
        matches!(&events[1], AuditEvent::Image { origin, destination, operator_public: false, .. }
        if origin == "workspace:data/scan.png" && destination == "local")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_run_sends_its_images_again_from_the_store() {
    let w = Ws::new();
    let e = engine(&w, policy(ToFrontier::Public, false), vec![]);
    let cfg = config(&w, images(true, vec![]));
    let first = Frontier::new(vec![
        Step::Call("read_file", json!({"path": "docs/ui.png"})),
        Step::Fail(400),
    ]);
    let terminal = run(&w, &cfg, e.as_ref(), Some(&e), &first, false).await;
    assert!(matches!(terminal, Terminal::Failed { .. }), "{terminal:?}");
    let ui = w.prepared("docs/ui.png");
    let transcript = std::fs::read_to_string(w.run_dir().join("transcript.jsonl")).unwrap();
    assert!(transcript.contains(&ui.sha256) && !transcript.contains(&ui.base64()));
    drop(e);

    // A new process: the engine is reopened on the run directory.
    let e = Engine::open(&w.run_dir(), policy(ToFrontier::Public, false), None).unwrap();
    let second = Frontier::new(vec![Step::Call("finish", json!({"summary": "done"}))]);
    let terminal = run(&w, &cfg, e.as_ref(), Some(&e), &second, true).await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    // The resumed request carries the same conversation, image included,
    // byte for byte: the provider's prefix cache still applies.
    let before: Value = serde_json::from_str(&first.bodies()[1]).unwrap();
    let after: Value = serde_json::from_str(&second.bodies()[0]).unwrap();
    assert_eq!(before["messages"], after["messages"]);
    assert!(second.bodies()[0].contains(&ui.data_url()));

    // A stored image that no longer matches its digest is not sent.
    let stored = w.run_dir().join(format!("images/{}.png", ui.sha256));
    std::fs::write(&stored, b"tampered").unwrap();
    let third = Frontier::new(vec![Step::Call("finish", json!({"summary": "done"}))]);
    // (The run completed; resume a copy of its transcript without the end.)
    let transcript = std::fs::read_to_string(w.run_dir().join("transcript.jsonl")).unwrap();
    let open: String = transcript
        .lines()
        .filter(|l| !l.contains("\"kind\":\"end\""))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(w.run_dir().join("transcript.jsonl"), open).unwrap();
    let terminal = run(&w, &cfg, e.as_ref(), Some(&e), &third, true).await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    assert!(!third.bodies()[0].contains("data:image/"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_attaches_images_to_the_next_message_and_resumes_with_them() {
    let w = Ws::new();
    let e = engine(&w, policy(ToFrontier::Never, false), vec![]);
    let cfg = config(&w, images(true, vec![]));
    let frontier = Frontier::new(vec![
        Step::Text("The button overlaps the label."),
        Step::Text("Still there."),
    ]);
    let gated = gated(&w, &frontier, Some(&e));
    let git = duet_git::Git::locate().unwrap();
    let limits = SessionLimits {
        frontier_usd: 10.0,
        working_time: Duration::from_secs(600),
    };
    let interrupted = Arc::new(AtomicBool::new(false));
    let mut session = Session::open(
        &cfg,
        &gated,
        e.as_ref(),
        &git,
        interrupted.clone(),
        limits,
        false,
    )
    .unwrap();
    // Not public, and no local model reads images: refused at once.
    let refused = session
        .attach(w.path("outside/photo.png"), false)
        .unwrap_err();
    assert!(refused.contains("local.vision is false"), "{refused}");
    let said = session.attach(w.path("outside/photo.png"), true).unwrap();
    assert!(said.contains("the frontier will see the image"), "{said}");
    assert_eq!(session.attached().len(), 1);
    let end = session.turn("What is wrong in this screenshot?").await;
    assert!(matches!(end, TurnEnd::Replied { .. }), "{end:?}");
    assert!(session.attached().is_empty());
    let photo = w.prepared("outside/photo.png");
    assert_eq!(frontier.bodies()[0].matches(&photo.data_url()).count(), 1);
    let _ = session.end(false);

    let mut resumed =
        Session::open(&cfg, &gated, e.as_ref(), &git, interrupted, limits, true).unwrap();
    let end = resumed.turn("And now?").await;
    assert!(matches!(end, TurnEnd::Replied { .. }), "{end:?}");
    let body: Value = serde_json::from_str(&frontier.bodies()[1]).unwrap();
    assert_eq!(
        body["messages"][1]["content"][0]["image_url"]["url"],
        photo.data_url()
    );
    assert!(
        body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("And now?")
    );
}

/// `duet run` with the binary: the given images are checked before the run
/// exists (hybrid defaults: no local vision, images.to_frontier = "never").
#[test]
fn duet_run_refuses_an_unusable_attachment_before_the_run_starts() {
    let w = Ws::new();
    let home = w.root.join("owner");
    std::fs::create_dir_all(&home).unwrap();
    let duet = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_duet"))
            .args(args)
            .arg("--workspace")
            .arg(&w.ws)
            .env("DUET_CONFIG_HOME", &home)
            .env_remove("DUET_LOCAL_PORTS")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    };
    let photo = w.path("outside/photo.png");
    let scan = w.path("data/scan.png");
    let lib = w.path("src/lib.rs");
    for (args, why) in [
        (
            vec!["run", "--image", photo.to_str().unwrap(), "Fix it."],
            "local.vision is false",
        ),
        (
            vec!["run", "--image-public", scan.to_str().unwrap(), "Fix it."],
            "sensitive path",
        ),
        (
            vec!["run", "--image-public", photo.to_str().unwrap(), "Fix it."],
            "frontier.vision is false",
        ),
        (
            vec!["run", "--image", lib.to_str().unwrap(), "Fix it."],
            "cannot be read as an image",
        ),
    ] {
        let out = duet(&args);
        let text = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(!out.status.success(), "{args:?}: {text}");
        assert!(text.contains(why), "{args:?}: {text}");
        assert!(!w.ws.join(".duet/runs").exists(), "a run was created");
    }
}
