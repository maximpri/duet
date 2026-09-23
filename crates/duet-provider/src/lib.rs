// SPDX-License-Identifier: GPL-3.0-or-later
//! Model API layer: wire dialects, streaming, retry, credentials, usage and pricing.

pub mod chat;
pub mod client;
pub mod endpoint;
pub mod error;
pub mod price;
pub mod probe;
pub mod recover;
pub mod retry;
pub mod sse;
pub mod types;

pub use client::{ChatProvider, ProviderConfig, Role};
pub use error::{ErrorKind, ProviderError};
pub use types::{Item, Request, Response, StopReason, ToolCall, ToolSpec, Usage};

#[cfg(test)]
mod tests;
