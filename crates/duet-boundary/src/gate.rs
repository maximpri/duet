// SPDX-License-Identifier: GPL-3.0-or-later
//! The outbound gate: the only code path from Duet to the frontier.
//!
//! The agent never holds a provider. It holds a [`GatedFrontier`], which can
//! only be built by [`OutboundGate::wrap`]; every request is passed through the
//! gate's filters, and the exact bytes sent are appended to the audit log.

use crate::audit::{AuditEvent, AuditHandle, AuditLog};
use duet_fs::FsError;
use duet_provider::chat::build_body;
use duet_provider::{ChatProvider, ProviderError, Request, Response};

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
    /// `Err(reason)` blocks the request.
    fn check(&self, body: &serde_json::Value) -> Result<(), String>;
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
        }
    }
}

pub struct GatedFrontier {
    gate: OutboundGate,
    provider: ChatProvider,
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

    /// Filters, checks, audits, then sends.
    pub async fn create(&self, request: &Request) -> Result<(Response, Vec<String>), GateError> {
        let mut outbound = request.clone();
        let mut interventions = Vec::new();
        for f in &self.gate.filters {
            interventions.extend(
                f.apply(&mut outbound)
                    .into_iter()
                    .map(|d| format!("{}: {d}", f.name())),
            );
        }
        let cfg = self.provider.config();
        let body = build_body(&cfg.model, &outbound, true);
        for c in &self.gate.checks {
            if let Err(reason) = c.check(&body) {
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
