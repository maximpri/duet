// SPDX-License-Identifier: GPL-3.0-or-later
//! The outbound gate: the only code path from Duet to the frontier.
//!
//! The agent never holds a provider. It holds a [`GatedFrontier`], which can
//! only be built by [`OutboundGate::wrap`]; every request is passed through the
//! gate's filters, and the exact bytes sent are appended to the audit log.

use crate::audit::AuditLog;
use duet_fs::FsError;
use duet_provider::chat::build_body;
use duet_provider::{ChatProvider, ProviderError, Request, Response};
use std::sync::Mutex;

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
    audit: Mutex<AuditLog>,
}

impl OutboundGate {
    pub fn new(audit: AuditLog) -> Self {
        Self {
            filters: Vec::new(),
            checks: Vec::new(),
            audit: Mutex::new(audit),
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
                return Err(GateError::Blocked {
                    filter: c.name(),
                    reason,
                });
            }
        }
        self.gate.audit.lock().expect("audit lock").append(
            &cfg.base_url,
            &cfg.model,
            body,
            interventions.clone(),
        )?;
        let response = self.provider.create(&outbound).await?;
        Ok((response, interventions))
    }
}
