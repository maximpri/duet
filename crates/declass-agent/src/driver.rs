// SPDX-License-Identifier: GPL-3.0-or-later
//! The model that drives a loop: it answers each request of the conversation
//! with text and tool calls.
//!
//! The run's loop and sub-agents' loops take the model as a parameter, so a
//! loop is not tied to one kind of model. The frontier loops are driven by a
//! [`GatedFrontier`] (the run's frontier, or `subagents.model` behind its own
//! gate on the same audit log); the local explorer's read-only loop in the
//! secure zone (`crate::explore`) by a [`LocalAgent`], whose requests go to
//! the local endpoint only and need no outbound gate.

use declass_boundary::audit::AuditHandle;
use declass_boundary::local::LocalAgent;
use declass_boundary::model::{Request, Response};
use declass_boundary::{GateError, GatedFrontier};
use futures_util::future::BoxFuture;
use tokio::time::Instant;

pub trait Driver: Send + Sync {
    /// The model id (transcripts and audit events name it).
    fn model(&self) -> &str;
    /// When the driver stops retrying on its own, if it has such a deadline.
    fn deadline(&self) -> Option<Instant>;
    /// The run's audit log, which the driver's requests are recorded in.
    fn audit(&self) -> &AuditHandle;
    /// One request: the response and what the outbound filters changed.
    fn create<'a>(
        &'a self,
        request: &'a Request,
    ) -> BoxFuture<'a, Result<(Response, Vec<String>), GateError>>;
    /// `request` in the form it would be sent: after the outbound filters,
    /// neither checked, audited nor sent (context compaction reads this).
    fn as_sent(&self, request: &Request) -> Request;
}

impl Driver for GatedFrontier {
    fn model(&self) -> &str {
        GatedFrontier::model(self)
    }

    fn deadline(&self) -> Option<Instant> {
        GatedFrontier::deadline(self)
    }

    fn audit(&self) -> &AuditHandle {
        GatedFrontier::audit(self)
    }

    fn create<'a>(
        &'a self,
        request: &'a Request,
    ) -> BoxFuture<'a, Result<(Response, Vec<String>), GateError>> {
        Box::pin(GatedFrontier::create(self, request))
    }

    fn as_sent(&self, request: &Request) -> Request {
        GatedFrontier::as_sent(self, request)
    }
}

impl Driver for LocalAgent {
    fn model(&self) -> &str {
        LocalAgent::model(self)
    }

    fn deadline(&self) -> Option<Instant> {
        LocalAgent::deadline(self)
    }

    fn audit(&self) -> &AuditHandle {
        LocalAgent::audit(self)
    }

    fn create<'a>(
        &'a self,
        request: &'a Request,
    ) -> BoxFuture<'a, Result<(Response, Vec<String>), GateError>> {
        Box::pin(async move {
            LocalAgent::create(self, request)
                .await
                .map(|r| (r, Vec::new()))
                .map_err(GateError::Provider)
        })
    }

    /// Nothing filters a local request: it is sent as it is.
    fn as_sent(&self, request: &Request) -> Request {
        request.clone()
    }
}
