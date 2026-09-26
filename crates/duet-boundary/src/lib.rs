// SPDX-License-Identifier: GPL-3.0-or-later
//! Security engine: classification, transformation, vault, handles, bulky
//! offload, IP levels, local roles, outbound gate and audit.
//!
//! In passthrough mode only the gate and the audit log are active; hybrid mode
//! adds the engine ([`engine::Engine`]) as the presenter and outbound filter.

pub mod audit;
pub mod bulky;
pub mod detect;
pub mod engine;
pub mod gate;
pub mod handles;
pub mod images;
pub mod ip;
pub mod live;
pub mod local;
pub mod local_eval;
pub mod overlap;
pub mod pii;
pub mod policy;
pub mod probing;
pub mod reencoded;
pub mod rules;
pub mod skeleton;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod vault;
pub mod view;

pub use gate::{GateError, GatedFrontier, OutboundCheck, OutboundFilter, OutboundGate};

/// Types the agent needs, re-exported so the agent crate does not depend on
/// the provider crate (and so cannot construct an ungated provider).
pub mod model {
    pub use duet_provider::image::{
        DEFAULT_MAX_SIDE, MAX_INPUT_BYTES, has_image_extension, prepare as prepare_image, sniff,
        solid_png,
    };
    pub use duet_provider::types::{
        Image, Item, Request, Response, StopReason, ToolCall, ToolSpec, Usage,
    };
    pub use duet_provider::{ErrorKind, ProviderError};
}
