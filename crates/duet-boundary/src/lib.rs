// SPDX-License-Identifier: GPL-3.0-or-later
//! Security engine: classification, transformation, vault, handles, outbound gate and audit.
//!
//! M2 provides the gate and the audit log in pass-through mode; classification,
//! placeholders, handles and the local roles arrive in M3.

pub mod audit;
pub mod detect;
pub mod gate;
pub mod overlap;
pub mod vault;
pub mod view;

pub use gate::{GateError, GatedFrontier, OutboundCheck, OutboundFilter, OutboundGate};

/// Types the agent needs, re-exported so the agent crate does not depend on
/// the provider crate (and so cannot construct an ungated provider).
pub mod model {
    pub use duet_provider::types::{
        Item, Request, Response, StopReason, ToolCall, ToolSpec, Usage,
    };
    pub use duet_provider::{ErrorKind, ProviderError};
}
