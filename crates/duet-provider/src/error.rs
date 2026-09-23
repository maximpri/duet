// SPDX-License-Identifier: GPL-3.0-or-later
//! Provider errors and their retry classification.

use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// Connection refused, reset, DNS, TLS: nothing reached the model.
    Transport,
    /// Non-success HTTP status.
    Status(u16),
    /// The provider reported an error inside a successful stream.
    StreamError,
    /// The stream ended before a terminal event.
    Truncated,
    /// No first byte, or no progress, within the configured deadline.
    Timeout,
    /// The response could not be understood.
    Malformed,
    /// Credentials are missing or rejected.
    Auth,
    /// The request exceeded the model's context window.
    ContextOverflow,
    /// The endpoint is not allowed for this role (e.g. a cloud host as "local").
    Forbidden,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct ProviderError {
    pub kind: ErrorKind,
    pub message: String,
    /// Whether any model output had arrived when the error occurred.
    pub output_started: bool,
    /// Bytes of output received before the failure, for usage estimation.
    pub partial_output_bytes: usize,
    /// Server-requested delay before retrying.
    pub retry_after: Option<Duration>,
}

impl ProviderError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            output_started: false,
            partial_output_bytes: 0,
            retry_after: None,
        }
    }

    /// Transient failures that a fresh attempt can fix.
    pub fn is_retryable(&self) -> bool {
        match self.kind {
            ErrorKind::Transport
            | ErrorKind::StreamError
            | ErrorKind::Truncated
            | ErrorKind::Timeout => true,
            ErrorKind::Status(code) => {
                matches!(code, 408 | 409 | 425 | 429 | 500 | 502 | 503 | 504 | 529)
            }
            ErrorKind::Malformed
            | ErrorKind::Auth
            | ErrorKind::ContextOverflow
            | ErrorKind::Forbidden => false,
        }
    }
}

/// Classifies an HTTP error body that reports a context overflow.
pub fn is_context_overflow(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    [
        "context length",
        "context window",
        "maximum context",
        "too many tokens",
        "prompt is too long",
        "exceeds the context",
    ]
    .iter()
    .any(|p| m.contains(p))
}
