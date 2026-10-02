// SPDX-License-Identifier: GPL-3.0-or-later
//! Model API layer: wire dialects (Chat Completions, Anthropic Messages, OpenAI
//! Responses), streaming, retry, credentials, local-endpoint trust, usage and pricing.

pub mod anthropic;
pub mod backends;
pub mod catalog;
pub mod chat;
pub mod client;
pub mod dialect;
pub mod endpoint;
pub mod error;
pub mod image;
pub mod live;
pub mod meter;
#[cfg(any(test, feature = "test-support"))]
pub mod mock_http;
pub mod price;
pub mod probe;
pub mod recover;
pub mod responses;
pub mod retry;
pub mod sse;
pub mod types;

pub use client::{ChatProvider, ProviderConfig, Role};
pub use dialect::Dialect;
pub use error::{ErrorKind, ProviderError};
pub use types::{Image, Item, Request, Response, StopReason, ToolCall, ToolSpec, Usage};

#[cfg(test)]
mod tests;
