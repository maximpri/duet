// SPDX-License-Identifier: GPL-3.0-or-later
//! Model API layer: Chat Completions, streaming, retry, credentials, local-endpoint
//! trust, usage and pricing.

pub mod backends;
pub mod chat;
pub mod client;
pub mod endpoint;
pub mod error;
#[cfg(any(test, feature = "test-support"))]
pub mod mock_http;
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
