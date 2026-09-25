// SPDX-License-Identifier: GPL-3.0-or-later
//! The model that drives a loop: it answers each request of the conversation
//! with text and tool calls.
//!
//! The run's loop and sub-agents' loops take the model as a parameter, so a
//! loop is not tied to one kind of model. Today every driver is a
//! [`GatedFrontier`] (the run's frontier, or `subagents.model` behind its own
//! gate on the same audit log); a local model driving a read-only loop in the
//! secure zone can be another driver without changing the loop.

use duet_boundary::audit::AuditHandle;
use duet_boundary::model::{Request, Response};
use duet_boundary::{GateError, GatedFrontier};
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
}
