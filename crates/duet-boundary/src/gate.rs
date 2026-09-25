// SPDX-License-Identifier: GPL-3.0-or-later
//! The outbound gate: the only code path from Duet to the frontier.
//!
//! The agent never holds a provider. It holds a [`GatedFrontier`], which can
//! only be built by [`OutboundGate::wrap`]; every request is passed through the
//! gate's filters, and the exact bytes sent are appended to the audit log
//! (images as digests of their data).

use crate::audit::{AuditEvent, AuditHandle, AuditLog};
use duet_fs::FsError;
use duet_provider::{ChatProvider, Item, ProviderError, Request, Response};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A transformation applied to every outbound request before it is sent.
/// Returns descriptions of what it changed (empty when nothing changed).
pub trait OutboundFilter: Send + Sync {
    fn name(&self) -> &'static str;
    fn apply(&self, request: &mut Request) -> Vec<String>;
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
    /// image data replaced by digests (see [`duet_provider::image::redact`]).
    fn check(&self, body: &serde_json::Value) -> Result<(), String>;
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

    /// Filters, checks, audits, then sends. Whatever the dialect, the filters
    /// and checks see Duet's own request and the exact body that is sent. If
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
        let cfg = self.provider.config();
        // Checks and the audit log see the body with each image's data
        // replaced by its digest: image data is not text, and the log never
        // holds it. The provider sends the body with the images.
        let (body, images) = duet_provider::image::redact(&self.provider.body(&outbound));
        for c in &self.gate.checks {
            if let Err(reason) = c.check(&body).and_then(|()| c.check_images(&images)) {
                self.gate.audit.record(AuditEvent::BlockedSend {
                    check: c.name().to_owned(),
                });
                return Err(GateError::Blocked {
                    filter: c.name(),
                    reason,
                });
            }
        }
        self.gate
            .audit
            .append(&cfg.base_url, &cfg.model, body, interventions.clone())?;
        let response = self.provider.create(&outbound).await?;
        Ok((response, interventions))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{Line, read};
    use duet_provider::client::{HttpReply, Transport};
    use duet_provider::{ErrorKind, ProviderConfig, Role};
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
        let img =
            duet_provider::image::prepare(&duet_provider::image::solid_png(16, 16, [7, 7, 7]), 64)
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
        assert!(log.contains(&duet_provider::image::marker(&img.sha256, img.data().len())));
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
}
