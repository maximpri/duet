// SPDX-License-Identifier: GPL-3.0-or-later
//! Cost from the provider's own numbers.
//!
//! Token usage is recovered from the response bodies the leak proxy captured,
//! so every lane — Duet or external — is priced the same way. Prices come from
//! `pricing.toml`; an unverified price makes the cost unknown rather than zero.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Normalized usage: `uncached_input` excludes cache reads and writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub uncached_input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.uncached_input += other.uncached_input;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.output += other.output;
    }
}

#[derive(Default)]
struct Raw {
    prompt_tokens: Option<u64>,      // OpenAI chat: includes cached
    input_tokens: Option<u64>,       // Anthropic: excludes cache; Responses: includes cached
    cached_tokens: Option<u64>,      // OpenAI-style cached detail
    cache_read_input: Option<u64>,   // Anthropic
    cache_create_input: Option<u64>, // Anthropic
    output: Option<u64>,
}

fn max_opt(a: &mut Option<u64>, b: Option<u64>) {
    if let Some(b) = b {
        *a = Some(a.map_or(b, |a| a.max(b)));
    }
}

fn collect_usage(v: &Value, raw: &mut Raw) {
    match v {
        Value::Object(map) => {
            if let Some(Value::Object(u)) = map.get("usage") {
                let get = |k: &str| u.get(k).and_then(Value::as_u64);
                max_opt(&mut raw.prompt_tokens, get("prompt_tokens"));
                max_opt(&mut raw.input_tokens, get("input_tokens"));
                max_opt(&mut raw.cache_read_input, get("cache_read_input_tokens"));
                max_opt(
                    &mut raw.cache_create_input,
                    get("cache_creation_input_tokens"),
                );
                max_opt(
                    &mut raw.output,
                    get("completion_tokens").or_else(|| get("output_tokens")),
                );
                for detail in ["prompt_tokens_details", "input_tokens_details"] {
                    let cached = u
                        .get(detail)
                        .and_then(|d| d.get("cached_tokens"))
                        .and_then(Value::as_u64);
                    max_opt(&mut raw.cached_tokens, cached);
                }
                max_opt(&mut raw.cached_tokens, get("cached_tokens"));
            }
            for (k, child) in map {
                if k != "usage" {
                    collect_usage(child, raw);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect_usage(i, raw)),
        _ => {}
    }
}

/// Usage reported in one captured response body (plain JSON or SSE).
pub fn usage_from_response(body: &str) -> Option<Usage> {
    let mut raw = Raw::default();
    let mut any = false;
    let mut visit = |text: &str| {
        if let Ok(v) = serde_json::from_str::<Value>(text) {
            collect_usage(&v, &mut raw);
            any = true;
        }
    };
    if body.trim_start().starts_with('{') {
        visit(body);
    } else {
        for line in body.lines() {
            if let Some(data) = line.strip_prefix("data:") {
                visit(data.trim());
            }
        }
    }
    if !any || raw.output.is_none() {
        return None;
    }
    let output = raw.output.unwrap_or(0);
    let usage = if raw.cache_read_input.is_some() || raw.cache_create_input.is_some() {
        // Anthropic: input_tokens already excludes cache reads and writes.
        Usage {
            uncached_input: raw.input_tokens.unwrap_or(0),
            cache_read: raw.cache_read_input.unwrap_or(0),
            cache_write: raw.cache_create_input.unwrap_or(0),
            output,
        }
    } else {
        let total = raw.prompt_tokens.or(raw.input_tokens).unwrap_or(0);
        let cached = raw.cached_tokens.unwrap_or(0).min(total);
        Usage {
            uncached_input: total - cached,
            cache_read: cached,
            cache_write: 0,
            output,
        }
    };
    Some(usage)
}

/// The `model` field of a captured request body.
pub fn model_from_request(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .get("model")?
        .as_str()
        .map(str::to_owned)
}

#[derive(Debug, Default)]
pub struct ProxyUsage {
    pub by_model: BTreeMap<String, Usage>,
    /// Requests whose response carried no usage (streams cut short, errors).
    pub unreported_requests: u64,
}

/// Sums usage over every request/response pair a proxy session captured.
pub fn usage_from_proxy_log(log_dir: &Path) -> Result<ProxyUsage> {
    let mut out = ProxyUsage::default();
    let requests = crate::leakproxy::read_requests(log_dir)?;
    for r in requests {
        let req = fs::read_to_string(log_dir.join(format!("requests/{:05}.body", r.seq)))
            .unwrap_or_default();
        let resp = fs::read_to_string(log_dir.join(format!("responses/{:05}.body", r.seq)))
            .unwrap_or_default();
        let model = model_from_request(&req).unwrap_or_else(|| "unknown".into());
        match usage_from_response(&resp) {
            Some(u) => out.by_model.entry(model).or_default().add(u),
            None => out.unreported_requests += 1,
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Deserialize)]
pub struct Price {
    pub model: String,
    /// USD per million tokens.
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
    pub source: String,
    pub observed: String,
    pub verified: bool,
}

#[derive(Debug, Deserialize)]
pub struct PriceTable {
    #[serde(rename = "price")]
    pub prices: Vec<Price>,
    pub electricity_usd_per_kwh: f64,
}

impl PriceTable {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Ok(toml::from_str(&text)?)
    }

    /// Dollar cost of `usage` on `model`. Errors if the price is missing or unverified.
    pub fn cost(&self, model: &str, usage: Usage) -> Result<f64> {
        let Some(p) = self.prices.iter().find(|p| p.model == model) else {
            bail!("no price for model {model}");
        };
        if !p.verified {
            bail!("price for {model} is not verified ({})", p.source);
        }
        let m = 1_000_000.0;
        Ok(usage.uncached_input as f64 * p.input / m
            + usage.cache_read as f64 * p.cache_read / m
            + usage.cache_write as f64 * p.cache_write / m
            + usage.output as f64 * p.output / m)
    }

    /// Electricity cost of `busy_seconds` of local inference at `watts`.
    pub fn electricity(&self, watts: f64, busy_seconds: f64) -> f64 {
        watts * busy_seconds / 3_600_000.0 * self.electricity_usd_per_kwh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_chat_stream_usage() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
                    data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1000,\"completion_tokens\":50,\
                    \"prompt_tokens_details\":{\"cached_tokens\":800}}}\n\ndata: [DONE]\n\n";
        let u = usage_from_response(body).unwrap();
        assert_eq!(
            u,
            Usage {
                uncached_input: 200,
                cache_read: 800,
                cache_write: 0,
                output: 50
            }
        );
    }

    #[test]
    fn anthropic_stream_usage() {
        let body = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":\
                    {\"input_tokens\":30,\"cache_read_input_tokens\":900,\"cache_creation_input_tokens\":70,\
                    \"output_tokens\":1}}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\
                    \"usage\":{\"output_tokens\":120}}\n\n";
        let u = usage_from_response(body).unwrap();
        assert_eq!(
            u,
            Usage {
                uncached_input: 30,
                cache_read: 900,
                cache_write: 70,
                output: 120
            }
        );
    }

    #[test]
    fn plain_json_usage_and_missing_usage() {
        let body = r#"{"id":"x","usage":{"prompt_tokens":10,"completion_tokens":2}}"#;
        assert_eq!(usage_from_response(body).unwrap().uncached_input, 10);
        assert!(usage_from_response("data: {\"choices\":[]}\n\n").is_none());
    }

    #[test]
    fn prices_refuse_unverified_and_compute_cost() {
        let table: PriceTable = toml::from_str(
            r#"
electricity_usd_per_kwh = 0.20
[[price]]
model = "a"
input = 1.0
cache_read = 0.1
cache_write = 1.25
output = 4.0
source = "test"
observed = "2026-09-23"
verified = true
[[price]]
model = "b"
input = 1.0
cache_read = 0.1
cache_write = 1.0
output = 4.0
source = "placeholder"
observed = "2026-09-23"
verified = false
"#,
        )
        .unwrap();
        let u = Usage {
            uncached_input: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 0,
            output: 500_000,
        };
        assert!((table.cost("a", u).unwrap() - 3.1).abs() < 1e-9);
        assert!(table.cost("b", u).is_err());
        assert!(table.cost("zzz", u).is_err());
        assert!((table.electricity(100.0, 3600.0) - 0.02).abs() < 1e-12);
    }
}
