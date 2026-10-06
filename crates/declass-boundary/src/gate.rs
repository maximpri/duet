// SPDX-License-Identifier: GPL-3.0-or-later
//! The outbound gate: the only code path from Declass to the frontier.
//!
//! The agent never holds a provider. It holds a [`GatedFrontier`], which can
//! only be built by [`OutboundGate::wrap`]; every request is passed through the
//! gate's filters, and the exact bytes sent are appended to the audit log
//! (images as digests of their data).
//!
//! A request a check refuses after filtering is not sent. The filters then
//! withhold every part of it that holds what the checks refuse
//! ([`OutboundFilter::withhold`]); if the checks pass that, it is sent instead
//! and the audit log records a `send_withheld` event, so a part the filter
//! could not clean costs that part, not the run. Only a request that still
//! fails ends in [`GateError::Blocked`].

use crate::audit::{AuditEvent, AuditHandle, AuditLog};
use declass_fs::FsError;
use declass_provider::{ChatProvider, Item, ProviderError, Request, Response, ToolCall, ToolSpec};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A transformation applied to every outbound request before it is sent.
/// Returns descriptions of what it changed (empty when nothing changed).
pub trait OutboundFilter: Send + Sync {
    fn name(&self) -> &'static str;
    fn apply(&self, request: &mut Request) -> Vec<String>;
    /// The stricter pass, after a check refused the filtered request:
    /// replaces each part of it that still holds what the checks refuse (a
    /// message, a tool result, a call's arguments) by a marker, instead of
    /// rewriting it. Returns how many parts it withheld; zero when it found
    /// none (the request then stays blocked).
    fn withhold(&self, _request: &mut Request) -> usize {
        0
    }
}

/// The strings of a request body that are its framing, not its content: every
/// key and string value in the body of [`framing_request`] (the body's keys,
/// roles and block types, the model's name, call ids, tool names and
/// parameter schemas). A check may skip a string of the body that is framing:
/// a sensitive value that happens to spell one (a `.env` value `required`,
/// the model's name) is not disclosed by the wire format.
#[derive(Debug, Default)]
pub struct Framing(HashSet<String>);

impl Framing {
    /// The keys and strings of `body`, the body of a [`framing_request`].
    pub fn of(body: &Value) -> Self {
        fn collect(v: &Value, out: &mut HashSet<String>) {
            match v {
                Value::String(s) => {
                    out.insert(s.clone());
                }
                Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
                Value::Object(m) => m.iter().for_each(|(k, x)| {
                    out.insert(k.clone());
                    collect(x, out);
                }),
                _ => {}
            }
        }
        let mut out = HashSet::new();
        collect(body, &mut out);
        Self(out)
    }

    pub fn contains(&self, s: &str) -> bool {
        self.0.contains(s)
    }
}

/// What stands for each text in a [`framing_request`]: not empty, so every
/// dialect lays the request out as it does the real one, and too short to
/// hold a value the checks look for.
const BLANK: &str = "\u{0}";

/// `request` with all its content blanked: the system prompt, every text,
/// reasoning and tool result, tool descriptions, the name and arguments of
/// every call and replayed reasoning blocks. What its body still holds is the
/// wire format's own text and what is fixed for a run (tool names and
/// schemas, call ids, image digests, settings).
pub fn framing_request(request: &Request) -> Request {
    let blank = |s: &str| match s.is_empty() {
        true => String::new(),
        false => BLANK.to_owned(),
    };
    let items = request
        .items
        .iter()
        .map(|item| match item {
            Item::User { text } => Item::User { text: blank(text) },
            Item::ToolResult { call_id, content } => Item::ToolResult {
                call_id: call_id.clone(),
                content: blank(content),
            },
            Item::Assistant {
                text,
                reasoning,
                tool_calls,
                ..
            } => Item::Assistant {
                text: blank(text),
                reasoning: reasoning.as_deref().map(blank),
                tool_calls: tool_calls
                    .iter()
                    .map(|c| ToolCall {
                        id: c.id.clone(),
                        name: blank(&c.name),
                        arguments: serde_json::Map::new(),
                        raw_arguments: "{}".into(),
                    })
                    .collect(),
                replay: None,
            },
            Item::Images { .. } => item.clone(),
        })
        .collect();
    Request {
        system: blank(&request.system),
        items,
        tools: request
            .tools
            .iter()
            .map(|t| ToolSpec {
                name: t.name.clone(),
                description: blank(&t.description),
                parameters: t.parameters.clone(),
            })
            .collect(),
        max_output_tokens: request.max_output_tokens,
        temperature: request.temperature,
        response_schema: request.response_schema.clone(),
        extra: request.extra.clone(),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GateError {
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error("audit log: {0}")]
    Audit(#[from] FsError),
    #[error("request blocked by {filter}: {reason}")]
    Blocked {
        filter: &'static str,
        reason: String,
    },
}

/// A filter may refuse a request outright instead of rewriting it.
pub trait OutboundCheck: Send + Sync {
    fn name(&self) -> &'static str;
    /// `Err(reason)` blocks the request. `body` is the body as sent, with
    /// image data replaced by digests (see [`declass_provider::image::redact`]).
    fn check(&self, body: &serde_json::Value) -> Result<(), String>;
    /// [`Self::check`] with the body's [`Framing`], which the gate always
    /// gives. By default the framing is not used.
    fn check_framed(&self, body: &serde_json::Value, _framing: &Framing) -> Result<(), String> {
        self.check(body)
    }
    /// The digests of the images in the request, in order; `Err(reason)`
    /// blocks it.
    fn check_images(&self, _digests: &[String]) -> Result<(), String> {
        Ok(())
    }
}

pub struct OutboundGate {
    filters: Vec<Box<dyn OutboundFilter>>,
    checks: Vec<Box<dyn OutboundCheck>>,
    audit: AuditHandle,
}

impl OutboundGate {
    pub fn new(audit: AuditLog) -> Self {
        Self {
            filters: Vec::new(),
            checks: Vec::new(),
            audit: AuditHandle::new(audit),
        }
    }

    /// A gate that appends to a log another gate already writes (a second
    /// frontier model in the same run, such as sub-agents' `subagents.model`):
    /// both share one hash chain.
    pub fn with_audit(audit: AuditHandle) -> Self {
        Self {
            filters: Vec::new(),
            checks: Vec::new(),
            audit,
        }
    }

    pub fn with_filter(mut self, f: Box<dyn OutboundFilter>) -> Self {
        self.filters.push(f);
        self
    }

    pub fn with_check(mut self, c: Box<dyn OutboundCheck>) -> Self {
        self.checks.push(c);
        self
    }

    /// The only constructor of [`GatedFrontier`].
    pub fn wrap(self, provider: ChatProvider) -> GatedFrontier {
        GatedFrontier {
            gate: self,
            provider,
            replay_floor: AtomicUsize::new(0),
        }
    }
}

pub struct GatedFrontier {
    gate: OutboundGate,
    provider: ChatProvider,
    /// Items before this index are sent without their replayed reasoning: the
    /// provider refused it once (the history before it had changed, e.g. by
    /// context masking), so it is dropped from the front of the history for
    /// the rest of the run and the prefix stays the same from then on.
    replay_floor: AtomicUsize,
}

impl GatedFrontier {
    pub fn model(&self) -> &str {
        &self.provider.config().model
    }

    /// When the provider stops retrying (the run's wall-clock budget), if set.
    pub fn deadline(&self) -> Option<tokio::time::Instant> {
        self.provider.config().deadline
    }

    /// The run's audit log, for recording security events next to the requests.
    pub fn audit(&self) -> &AuditHandle {
        &self.gate.audit
    }

    /// `request` as the filters leave it: the form in which it is sent,
    /// without checking, auditing or sending it. What Declass gives another
    /// model of the conversation (context compaction's local summary) is
    /// taken from this form, never from the conversation as it is kept.
    pub fn as_sent(&self, request: &Request) -> Request {
        let mut outbound = request.clone();
        for f in &self.gate.filters {
            let _ = f.apply(&mut outbound);
        }
        outbound
    }

    /// Filters, checks, audits, then sends. Whatever the dialect, the filters
    /// and checks see Declass's own request and the exact body that is sent. If
    /// the provider refuses replayed reasoning, the request is sent once more
    /// without it (filtered, checked and audited again).
    pub async fn create(&self, request: &Request) -> Result<(Response, Vec<String>), GateError> {
        match self.send(request).await {
            Err(GateError::Provider(e)) if e.is_replay_rejected() => {
                self.replay_floor
                    .fetch_max(request.items.len(), Ordering::SeqCst);
                self.send(request).await
            }
            other => other,
        }
    }

    async fn send(&self, request: &Request) -> Result<(Response, Vec<String>), GateError> {
        let mut outbound = request.clone();
        let floor = self.replay_floor.load(Ordering::SeqCst);
        for item in outbound.items.iter_mut().take(floor) {
            if let Item::Assistant { replay, .. } = item {
                *replay = None;
            }
        }
        let mut interventions = Vec::new();
        for f in &self.gate.filters {
            interventions.extend(
                f.apply(&mut outbound)
                    .into_iter()
                    .map(|d| format!("{}: {d}", f.name())),
            );
        }
        let body = match self.checked(&outbound) {
            Ok(body) => body,
            Err((check, reason)) => {
                // What the filters left is withheld part by part and checked
                // again; a request is blocked only if that does not pass.
                let mut parts = 0;
                for f in &self.gate.filters {
                    let n = f.withhold(&mut outbound);
                    if n > 0 {
                        interventions.push(format!(
                            "{}: withheld {n} part(s) the {check} check refused",
                            f.name()
                        ));
                    }
                    parts += n;
                }
                let again = match parts {
                    0 => Err((
                        check,
                        format!("{reason}; no part of the request could be withheld instead"),
                    )),
                    _ => self.checked(&outbound).map_err(|(c, r)| {
                        (
                            c,
                            format!("{r}, even with {parts} part(s) of the request withheld"),
                        )
                    }),
                };
                match again {
                    Ok(body) => {
                        self.gate.audit.record(AuditEvent::SendWithheld {
                            check: check.to_owned(),
                            parts: u32::try_from(parts).unwrap_or(u32::MAX),
                        });
                        body
                    }
                    Err((filter, reason)) => {
                        self.gate.audit.record(AuditEvent::BlockedSend {
                            check: filter.to_owned(),
                        });
                        return Err(GateError::Blocked { filter, reason });
                    }
                }
            }
        };
        let cfg = self.provider.config();
        self.gate
            .audit
            .append(&cfg.base_url, &cfg.model, body, interventions.clone())?;
        // Watched by the operator's terminal when the caller set a tap; the
        // request and the response are the same either way.
        let tap = crate::live::current();
        let response = self.provider.create_with(&outbound, tap.as_deref()).await?;
        Ok((response, interventions))
    }

    /// The body the checks and the audit log see, or the first check that
    /// refuses it and why. They see it with each image's data replaced by its
    /// digest: image data is not text, and the log never holds it. The
    /// provider sends the body with the images.
    fn checked(&self, outbound: &Request) -> Result<Value, (&'static str, String)> {
        let (body, images) = declass_provider::image::redact(&self.provider.body(outbound));
        if self.gate.checks.is_empty() {
            return Ok(body);
        }
        let framing = Framing::of(
            &declass_provider::image::redact(&self.provider.body(&framing_request(outbound))).0,
        );
        for c in &self.gate.checks {
            if let Err(reason) = c
                .check_framed(&body, &framing)
                .and_then(|()| c.check_images(&images))
            {
                return Err((c.name(), reason));
            }
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{Line, read};
    use declass_provider::client::{HttpReply, Transport};
    use declass_provider::{ErrorKind, ProviderConfig, Role};
    use futures_util::future::BoxFuture;

    /// Never reached: blocked requests are not sent.
    struct Unreachable;
    impl Transport for Unreachable {
        fn post(
            &self,
            _url: String,
            _headers: Vec<(String, String)>,
            _body: Vec<u8>,
        ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
            Box::pin(async { Err(ProviderError::new(ErrorKind::Forbidden, "sent")) })
        }
    }

    struct Refuse;
    impl OutboundCheck for Refuse {
        fn name(&self) -> &'static str {
            "known-values"
        }
        fn check(&self, _body: &serde_json::Value) -> Result<(), String> {
            Err("a secret value from .env would have been sent".into())
        }
    }

    /// Records what it was sent and answers with an empty stop.
    #[derive(Clone, Default)]
    struct Recording(std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>);
    impl Transport for Recording {
        fn post(
            &self,
            _url: String,
            _headers: Vec<(String, String)>,
            body: Vec<u8>,
        ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
            use futures_util::StreamExt;
            self.0.lock().unwrap().push(body);
            let chunks: Vec<Result<bytes::Bytes, String>> = vec![
                Ok(bytes::Bytes::from_static(
                    b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                )),
                Ok(bytes::Bytes::from_static(b"data: [DONE]\n\n")),
            ];
            Box::pin(async move {
                Ok(HttpReply {
                    status: 200,
                    headers: vec![],
                    body: futures_util::stream::iter(chunks).boxed(),
                })
            })
        }
    }

    /// Sees the digests of the images in each request.
    struct Digests(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
    impl OutboundCheck for Digests {
        fn name(&self) -> &'static str {
            "images"
        }
        fn check(&self, body: &serde_json::Value) -> Result<(), String> {
            // Checks see digests, never image data.
            match body.to_string().contains("data:image/") {
                true => Err("image data reached a check".into()),
                false => Ok(()),
            }
        }
        fn check_images(&self, digests: &[String]) -> Result<(), String> {
            self.0.lock().unwrap().extend(digests.iter().cloned());
            Ok(())
        }
    }

    #[tokio::test]
    async fn images_are_sent_but_audited_and_checked_as_digests() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("audit.jsonl");
        let sent = Recording::default();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let provider = ChatProvider::new(
            ProviderConfig::new("https://f.example/v1", "m", Role::Frontier),
            Box::new(sent.clone()),
        )
        .unwrap();
        let gated = OutboundGate::new(AuditLog::open(&p).unwrap())
            .with_check(Box::new(Digests(seen.clone())))
            .wrap(provider);
        let img = declass_provider::image::prepare(
            &declass_provider::image::solid_png(16, 16, [7, 7, 7]),
            64,
        )
        .unwrap();
        let req = Request {
            items: vec![
                Item::User { text: "see".into() },
                Item::Images {
                    call_id: None,
                    images: vec![img.clone()],
                },
            ],
            ..Request::default()
        };
        gated.create(&req).await.unwrap();
        let wire = String::from_utf8(sent.0.lock().unwrap()[0].clone()).unwrap();
        assert!(wire.contains(&img.base64()), "the provider gets the image");
        assert_eq!(*seen.lock().unwrap(), vec![img.sha256.clone()]);
        let log = std::fs::read_to_string(&p).unwrap();
        assert!(
            !log.contains(&img.base64()),
            "the audit log never holds image data"
        );
        assert!(log.contains(&declass_provider::image::marker(
            &img.sha256,
            img.data().len()
        )));
        assert_eq!(
            crate::audit::verify(&p).unwrap(),
            crate::audit::Verification::Intact { records: 1 }
        );
    }

    #[tokio::test]
    async fn a_blocked_send_is_audited_by_check_name_only() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("audit.jsonl");
        let provider = ChatProvider::new(
            ProviderConfig::new("https://f.example/v1", "m", Role::Frontier),
            Box::new(Unreachable),
        )
        .unwrap();
        let gated = OutboundGate::new(AuditLog::open(&p).unwrap())
            .with_check(Box::new(Refuse))
            .wrap(provider);
        let err = gated.create(&Request::default()).await.unwrap_err();
        assert!(matches!(err, GateError::Blocked { .. }));
        let lines = read(&p).unwrap();
        assert_eq!(lines.len(), 1);
        let Line::Event(e) = &lines[0] else {
            panic!("expected an event")
        };
        assert_eq!(
            e.event,
            crate::audit::AuditEvent::BlockedSend {
                check: "known-values".into()
            }
        );
        assert!(!std::fs::read_to_string(&p).unwrap().contains(".env"));
    }

    const VALUE: &str = "s3cr3t-value";

    /// Refuses a body with [`VALUE`] in it; records the framing it is given.
    #[derive(Default)]
    struct NoValue(std::sync::Arc<std::sync::Mutex<Vec<bool>>>);
    impl OutboundCheck for NoValue {
        fn name(&self) -> &'static str {
            "no-value"
        }
        fn check(&self, body: &serde_json::Value) -> Result<(), String> {
            match body.to_string().contains(VALUE) {
                true => Err("the value would have been sent".into()),
                false => Ok(()),
            }
        }
        fn check_framed(&self, body: &serde_json::Value, framing: &Framing) -> Result<(), String> {
            // The framing holds the wire format's words, never the content.
            self.0
                .lock()
                .unwrap()
                .push(framing.contains("user") && !framing.contains(&format!("use {VALUE}")));
            self.check(body)
        }
    }

    /// Changes nothing, then withholds user text holding [`VALUE`] (or, with
    /// `leave`, claims to without doing it).
    struct Withholds {
        leave: bool,
    }
    impl OutboundFilter for Withholds {
        fn name(&self) -> &'static str {
            "withholds"
        }
        fn apply(&self, _request: &mut Request) -> Vec<String> {
            Vec::new()
        }
        fn withhold(&self, request: &mut Request) -> usize {
            let mut n = 0;
            for item in &mut request.items {
                if let Item::User { text } = item
                    && text.contains(VALUE)
                {
                    if !self.leave {
                        *text = "[withheld]".into();
                    }
                    n += 1;
                }
            }
            n
        }
    }

    fn events(p: &std::path::Path) -> Vec<AuditEvent> {
        read(p)
            .unwrap()
            .into_iter()
            .filter_map(|l| match l {
                Line::Event(e) => Some(e.event),
                Line::Request(_) => None,
            })
            .collect()
    }

    fn with_value() -> Request {
        Request {
            items: vec![
                Item::User {
                    text: format!("use {VALUE}"),
                },
                Item::User {
                    text: "and keep this".into(),
                },
            ],
            ..Request::default()
        }
    }

    #[tokio::test]
    async fn a_part_a_check_refuses_is_withheld_and_the_request_sent() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("audit.jsonl");
        let sent = Recording::default();
        let framed = NoValue::default();
        let provider = ChatProvider::new(
            ProviderConfig::new("https://f.example/v1", "m", Role::Frontier),
            Box::new(sent.clone()),
        )
        .unwrap();
        let seen = framed.0.clone();
        let gated = OutboundGate::new(AuditLog::open(&p).unwrap())
            .with_filter(Box::new(Withholds { leave: false }))
            .with_check(Box::new(framed))
            .wrap(provider);
        let (_, interventions) = gated.create(&with_value()).await.unwrap();
        let wire = String::from_utf8(sent.0.lock().unwrap()[0].clone()).unwrap();
        assert!(
            !wire.contains(VALUE) && wire.contains("and keep this"),
            "{wire}"
        );
        assert_eq!(
            interventions,
            ["withholds: withheld 1 part(s) the no-value check refused"]
        );
        assert_eq!(
            events(&p),
            [AuditEvent::SendWithheld {
                check: "no-value".into(),
                parts: 1
            }]
        );
        assert_eq!(*seen.lock().unwrap(), [true, true], "checked twice, framed");
        assert_eq!(
            crate::audit::verify(&p).unwrap(),
            crate::audit::Verification::Intact { records: 2 }
        );
    }

    #[tokio::test]
    async fn a_request_still_refused_after_withholding_is_blocked_with_why() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("audit.jsonl");
        let provider = ChatProvider::new(
            ProviderConfig::new("https://f.example/v1", "m", Role::Frontier),
            Box::new(Unreachable),
        )
        .unwrap();
        let gated = OutboundGate::new(AuditLog::open(&p).unwrap())
            .with_filter(Box::new(Withholds { leave: true }))
            .with_check(Box::new(NoValue::default()))
            .wrap(provider);
        let err = gated.create(&with_value()).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "request blocked by no-value: the value would have been sent, even with 1 part(s) \
of the request withheld"
        );
        assert_eq!(
            events(&p),
            [AuditEvent::BlockedSend {
                check: "no-value".into()
            }]
        );
    }
}
