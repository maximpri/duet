// SPDX-License-Identifier: GPL-3.0-or-later
//! Wire dialects: which API a frontier endpoint speaks. Every dialect builds
//! its body from Declass's own [`Request`], so outbound filters and checks work on
//! the same content whatever the dialect, and assembles its stream into the
//! same [`Response`].

use crate::anthropic::{self, AnthropicAssembler};
use crate::chat::{self, ChatAssembler};
use crate::error::ProviderError;
use crate::responses::{self, ResponsesAssembler};
use crate::types::{Request, Response, ToolSpec};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Dialect {
    /// OpenAI-compatible Chat Completions (`/chat/completions`).
    #[default]
    Chat,
    /// Anthropic Messages (`/messages`).
    Anthropic,
    /// OpenAI Responses (`/responses`).
    Responses,
}

impl Dialect {
    /// The names `frontier.dialect` accepts.
    pub const NAMES: [&'static str; 3] = ["chat", "anthropic", "responses"];

    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "chat" => Some(Self::Chat),
            "anthropic" => Some(Self::Anthropic),
            "responses" => Some(Self::Responses),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Anthropic => anthropic::DIALECT,
            Self::Responses => responses::DIALECT,
        }
    }

    /// Path of the generation endpoint under the base URL.
    pub fn path(self) -> &'static str {
        match self {
            Self::Chat => "/chat/completions",
            Self::Anthropic => "/messages",
            Self::Responses => "/responses",
        }
    }

    /// The request body exactly as it is sent.
    pub fn build_body(self, model: &str, req: &Request, stream: bool) -> Value {
        match self {
            Self::Chat => chat::build_body(model, req, stream),
            Self::Anthropic => anthropic::build_body(model, req, stream),
            Self::Responses => responses::build_body(model, req, stream),
        }
    }

    /// Credential headers for an API key.
    pub fn auth_headers(self, key: &str) -> Vec<(String, String)> {
        match self {
            Self::Anthropic => vec![("x-api-key".to_owned(), key.to_owned())],
            Self::Chat | Self::Responses => {
                vec![("authorization".to_owned(), format!("Bearer {key}"))]
            }
        }
    }

    /// Headers every request of this dialect carries.
    pub fn fixed_headers(self) -> Vec<(String, String)> {
        match self {
            Self::Anthropic => vec![(
                "anthropic-version".to_owned(),
                anthropic::API_VERSION.to_owned(),
            )],
            Self::Chat | Self::Responses => Vec::new(),
        }
    }

    pub fn assembler(self) -> Assembler {
        match self {
            Self::Chat => Assembler::Chat(ChatAssembler::default()),
            Self::Anthropic => Assembler::Anthropic(AnthropicAssembler::default()),
            Self::Responses => Assembler::Responses(ResponsesAssembler::default()),
        }
    }
}

/// A stream assembler for one dialect.
pub enum Assembler {
    Chat(ChatAssembler),
    Anthropic(AnthropicAssembler),
    Responses(ResponsesAssembler),
}

impl Assembler {
    /// Applies one SSE `data:` payload.
    pub fn apply(&mut self, data: &str) -> Result<(), ProviderError> {
        match self {
            Self::Chat(a) => a.apply(data),
            Self::Anthropic(a) => a.apply(data),
            Self::Responses(a) => a.apply(data),
        }
    }

    pub fn output_bytes(&self) -> usize {
        match self {
            Self::Chat(a) => a.output_bytes(),
            Self::Anthropic(a) => a.output_bytes(),
            Self::Responses(a) => a.output_bytes(),
        }
    }

    /// What the stream holds so far, part by part (see [`crate::live`]).
    pub fn view(&self) -> Vec<(usize, crate::live::Part<'_>)> {
        match self {
            Self::Chat(a) => a.view(),
            Self::Anthropic(a) => a.view(),
            Self::Responses(a) => a.view(),
        }
    }

    /// Completes the response. Text tool-call recovery (`recover_from`)
    /// applies to Chat Completions only, which local servers speak.
    pub fn finish(
        self,
        requested_model: &str,
        recover_from: Option<&[ToolSpec]>,
    ) -> Result<Response, ProviderError> {
        match self {
            Self::Chat(a) => a.finish(requested_model, recover_from),
            Self::Anthropic(a) => a.finish(requested_model),
            Self::Responses(a) => a.finish(requested_model),
        }
    }
}

/// The reasoning effort Declass asks for is carried in `extra` as
/// `reasoning_effort` (the Chat Completions field). Other dialects move it to
/// their own field; the remaining entries are merged into the body as given.
pub(crate) fn split_extra(extra: &Map<String, Value>) -> (Option<Value>, Map<String, Value>) {
    let mut rest = extra.clone();
    let effort = rest.remove("reasoning_effort");
    (effort, rest)
}

/// Tools sorted by name, so the request prefix is byte-stable across turns.
pub(crate) fn sorted_tools(tools: &[ToolSpec]) -> Vec<&ToolSpec> {
    let mut sorted: Vec<&ToolSpec> = tools.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    sorted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for name in Dialect::NAMES {
            assert_eq!(Dialect::parse(name).unwrap().as_str(), name);
        }
        assert_eq!(Dialect::parse(" Anthropic "), Some(Dialect::Anthropic));
        assert_eq!(Dialect::parse("messages"), None);
        assert_eq!(Dialect::default(), Dialect::Chat);
    }

    #[test]
    fn each_dialect_authenticates_its_own_way() {
        assert_eq!(Dialect::Anthropic.auth_headers("k")[0].0, "x-api-key");
        assert_eq!(Dialect::Chat.auth_headers("k")[0].1, "Bearer k");
        assert_eq!(Dialect::Responses.auth_headers("k")[0].1, "Bearer k");
        assert_eq!(
            Dialect::Anthropic.fixed_headers()[0],
            ("anthropic-version".to_owned(), "2023-06-01".to_owned())
        );
    }
}
