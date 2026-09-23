// SPDX-License-Identifier: GPL-3.0-or-later
//! Dialect-neutral request and response types.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A tool the model may call. Tools are always sent sorted by name so the
/// request prefix stays byte-identical across turns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments object.
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Parsed arguments; always a JSON object.
    pub arguments: Map<String, Value>,
    /// Arguments exactly as the model produced them, replayed verbatim so the
    /// conversation prefix never changes.
    pub raw_arguments: String,
}

/// One conversation item. The conversation is append-only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Item {
    User {
        text: String,
    },
    Assistant {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    ToolResult {
        call_id: String,
        content: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub system: String,
    pub items: Vec<Item>,
    pub tools: Vec<ToolSpec>,
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f32>,
    /// Constrain the reply to a JSON Schema (structured output).
    pub response_schema: Option<Value>,
    /// Provider-specific top-level fields merged into the request body.
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Stop,
    ToolCalls,
    /// Output hit the length limit. Tool calls in such a response must not run.
    Length,
    ContentFilter,
    Other(String),
}

impl StopReason {
    pub fn from_wire(s: &str) -> Self {
        match s {
            "stop" | "end_turn" | "stop_sequence" | "completed" => Self::Stop,
            "tool_calls" | "function_call" | "tool_use" => Self::ToolCalls,
            "length" | "max_tokens" | "max_output_tokens" | "incomplete" => Self::Length,
            "content_filter" | "refusal" | "sensitive" => Self::ContentFilter,
            other => Self::Other(other.to_owned()),
        }
    }

    /// Whether tool calls in the response may be executed.
    pub fn allows_tools(&self) -> bool {
        matches!(self, Self::Stop | Self::ToolCalls)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageStatus {
    #[default]
    Reported,
    /// The provider sent no usage; counts are estimated from bytes.
    Estimated,
}

/// Token usage for one response. `input` excludes cache reads and writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    /// Reasoning tokens, already included in `output`.
    pub reasoning: u64,
    pub status: UsageStatus,
}

impl Usage {
    pub fn total_input(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }
}

/// Usage across every attempt of one logical request. Failed attempts never
/// poison the total: attempts that failed before producing output bill nothing,
/// and attempts that failed mid-stream are estimated and kept separately.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptUsage {
    pub billed: Usage,
    pub estimated_failed: Usage,
    pub attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: Option<String>,
    pub model: String,
    pub text: String,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub stop: StopReason,
    pub usage: Usage,
    pub attempts: AttemptUsage,
}

impl Response {
    /// The assistant item to append to the conversation.
    pub fn to_item(&self) -> Item {
        Item::Assistant {
            text: self.text.clone(),
            reasoning: self.reasoning.clone(),
            tool_calls: self.tool_calls.clone(),
        }
    }
}

/// Estimates tokens from bytes (roughly 3.5 bytes per token for code and prose).
pub fn estimate_tokens(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(7) * 2
}
