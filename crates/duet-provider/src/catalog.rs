// SPDX-License-Identifier: GPL-3.0-or-later
//! OpenRouter's public token-price catalog. No prompts or credentials are sent.

use crate::{price::Price, types::Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

pub const URL: &str = "https://openrouter.ai/api/v1/models";
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    pub fetched_at: u64,
    pub data: Vec<Model>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub pricing: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub price: Price,
    pub fetched_at: u64,
    /// Raw conditional rates, preserving OpenRouter's order and units.
    pub overrides: Vec<Value>,
    /// Owner supplied rates carry their own provenance in persisted runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

fn rate(v: &Value) -> Option<f64> {
    let n = v
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| v.as_f64())?;
    (n.is_finite() && (0.0..=1.0).contains(&n)).then_some(n * 1_000_000.0)
}

fn field(v: &Value, key: &str, fallback: f64) -> Option<f64> {
    match v.get(key) {
        None | Some(Value::Null) => Some(fallback),
        Some(v) => rate(v),
    }
}

impl Catalog {
    pub fn bundled() -> Self {
        serde_json::from_str(include_str!("../data/openrouter-pricing.json"))
            .expect("bundled OpenRouter catalog")
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("pricing catalog exceeds size limit".into());
        }
        let catalog: Self = serde_json::from_slice(bytes).map_err(|_| "invalid pricing catalog")?;
        if catalog.data.is_empty() || catalog.data.len() > 20_000 {
            return Err("invalid pricing catalog size".into());
        }
        Ok(catalog)
    }

    /// Exact slug first; an unqualified provider model is accepted only when
    /// its suffix is unique. Variants (:free, :batch, etc.) remain distinct.
    pub fn quote(&self, requested: &str) -> Option<Quote> {
        // A router's advertised zero is not the cost of the model it selects.
        if requested == "openrouter/auto" {
            return None;
        }
        let requested = match requested {
            "claude-opus-5-5" => "anthropic/claude-opus-5.5",
            "claude-sonnet-5-5" => "anthropic/claude-sonnet-5.5",
            other => other,
        };
        let model = self.data.iter().find(|m| m.id == requested).or_else(|| {
            if requested.contains('/') {
                return None;
            }
            let mut candidates = self.data.iter().filter(|m| {
                m.id.split_once('/')
                    .is_some_and(|(_, suffix)| suffix == requested)
            });
            let first = candidates.next()?;
            candidates.next().is_none().then_some(first)
        })?;
        if model.id == "openrouter/auto" {
            return None;
        }
        let p = &model.pricing;
        let input = rate(p.get("prompt")?)?;
        let output = rate(p.get("completion")?)?;
        // Fixed request fees cannot be inferred from aggregated token usage.
        if field(p, "request", 0.0)? != 0.0 {
            return None;
        }
        let discount = p.get("discount").and_then(Value::as_f64).unwrap_or(0.0);
        if !discount.is_finite() || !(0.0..=1.0).contains(&discount) {
            return None;
        }
        let overrides = match p.get("overrides") {
            None | Some(Value::Null) => vec![],
            Some(value) => value.as_array()?.clone(),
        };
        for tier in &overrides {
            // An unfamiliar condition must not silently become an unconditional
            // token rate. Non-text rate fields in the documented schema do not
            // affect our text-token estimate.
            if tier.as_object()?.keys().any(|key| {
                ![
                    "min_prompt_tokens",
                    "utc_start",
                    "utc_end",
                    "utc_days",
                    "prompt",
                    "completion",
                    "input_cache_read",
                    "input_cache_write",
                    "input_cache_write_1h",
                    "audio",
                    "input_audio_cache",
                    "image",
                    "output_audio",
                    "output_image",
                    "web_search",
                    "internal_reasoning",
                ]
                .contains(&key.as_str())
            }) {
                return None;
            }
            if let Some(threshold) = tier.get("min_prompt_tokens")
                && threshold.as_u64().is_none()
            {
                return None;
            }
            let window = [tier.get("utc_start"), tier.get("utc_end")];
            if window.iter().any(|v| v.is_some()) {
                for bound in window {
                    let n = bound?.as_u64()?;
                    if n > 2359 || n % 100 > 59 {
                        return None;
                    }
                }
            }
            if let Some(days) = tier.get("utc_days") {
                for day in days.as_array()? {
                    if !["mon", "tue", "wed", "thu", "fri", "sat", "sun"]
                        .iter()
                        .any(|d| day.as_str().is_some_and(|v| v.to_ascii_lowercase() == *d))
                    {
                        return None;
                    }
                }
            }
            for key in [
                "prompt",
                "completion",
                "input_cache_read",
                "input_cache_write",
            ] {
                field(tier, key, 0.0)?;
            }
        }
        Some(Quote {
            price: Price {
                model: model.id.clone(),
                input: input * (1.0 - discount),
                cache_read: field(p, "input_cache_read", input)? * (1.0 - discount),
                cache_write: field(p, "input_cache_write", input)? * (1.0 - discount),
                output: output * (1.0 - discount),
            },
            fetched_at: self.fetched_at,
            source: None,
            overrides: overrides
                .into_iter()
                .map(|mut t| {
                    if let Some(map) = t.as_object_mut() {
                        for key in [
                            "prompt",
                            "completion",
                            "input_cache_read",
                            "input_cache_write",
                        ] {
                            if let Some(v) = map.get_mut(key).filter(|v| !v.is_null()) {
                                *v = Value::from(rate(v).unwrap() / 1_000_000.0 * (1.0 - discount));
                            }
                        }
                    }
                    t
                })
                .collect(),
        })
    }

    pub fn known(&self) -> BTreeMap<String, Quote> {
        self.data
            .iter()
            .filter_map(|m| self.quote(&m.id).map(|q| (m.id.clone(), q)))
            .collect()
    }
}

impl Quote {
    pub fn cost(&self, usage: &Usage) -> f64 {
        self.cost_at(usage, time::OffsetDateTime::now_utc())
    }

    pub fn cost_at(&self, usage: &Usage, now: time::OffsetDateTime) -> f64 {
        let mut price = self.price.clone();
        let hhmm = u64::from(now.hour()) * 100 + u64::from(now.minute());
        let weekday = now.weekday().to_string().to_ascii_lowercase();
        for tier in &self.overrides {
            if tier
                .get("min_prompt_tokens")
                .and_then(Value::as_u64)
                .is_some_and(|n| usage.total_input() <= n)
            {
                continue;
            }
            if let Some(days) = tier.get("utc_days").and_then(Value::as_array)
                && !days.iter().any(|d| {
                    d.as_str()
                        .is_some_and(|d| weekday.starts_with(&d.to_ascii_lowercase()))
                })
            {
                continue;
            }
            if let (Some(start), Some(end)) = (
                tier.get("utc_start").and_then(Value::as_u64),
                tier.get("utc_end").and_then(Value::as_u64),
            ) {
                let applies = if start <= end {
                    hhmm >= start && hhmm < end
                } else {
                    hhmm >= start || hhmm < end
                };
                if !applies {
                    continue;
                }
            }
            price.input = field(tier, "prompt", price.input).unwrap_or(price.input);
            price.output = field(tier, "completion", price.output).unwrap_or(price.output);
            price.cache_read =
                field(tier, "input_cache_read", price.cache_read).unwrap_or(price.cache_read);
            price.cache_write =
                field(tier, "input_cache_write", price.cache_write).unwrap_or(price.cache_write);
        }
        price.cost(usage)
    }

    pub fn description(&self) -> String {
        if let Some(source) = &self.source {
            return format!(
                "{source} · {} · ${:.4} input / ${:.4} output / ${:.4} cache read / ${:.4} cache write per 1M tokens",
                self.price.model,
                self.price.input,
                self.price.output,
                self.price.cache_read,
                self.price.cache_write,
            );
        }
        let observed = time::OffsetDateTime::from_unix_timestamp(self.fetched_at as i64)
            .map(|t| t.date().to_string())
            .unwrap_or_else(|_| "unknown date".into());
        format!(
            "OpenRouter {} · {} · ${:.4} input / ${:.4} output / ${:.4} cache read / ${:.4} cache write per 1M tokens{}",
            self.price.model,
            observed,
            self.price.input,
            self.price.output,
            self.price.cache_read,
            self.price.cache_write,
            if self.overrides.is_empty() {
                ""
            } else {
                " (conditional tiers apply)"
            }
        )
    }
}

/// Fixed public endpoint, no authentication, no redirects, bounded transfer.
pub async fn fetch() -> Result<Catalog, String> {
    let endpoint = crate::endpoint::ApprovedEndpoint::new(URL, &crate::Role::Frontier)
        .map_err(|_| "invalid pricing endpoint")?;
    let client = crate::http::client(
        &endpoint,
        Duration::from_secs(8),
        Some(Duration::from_secs(8)),
    )
    .map_err(|_| "pricing client failed")?;
    let mut response = client
        .get(URL)
        .map_err(|_| "pricing recipient refused")?
        .send()
        .await
        .map_err(|_| "OpenRouter pricing unavailable")?
        .error_for_status()
        .map_err(|_| "OpenRouter pricing returned an error")?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "pricing transfer failed")?
    {
        if bytes.len() + chunk.len() > MAX_BYTES {
            return Err("pricing catalog exceeds size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let mut value: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid pricing JSON")?;
    value
        .as_object_mut()
        .ok_or("invalid pricing JSON object")?
        .insert(
            "fetched_at".into(),
            Value::from(time::OffsetDateTime::now_utc().unix_timestamp().max(0) as u64),
        );
    Catalog::parse(&serde_json::to_vec(&value).map_err(|_| "invalid pricing JSON")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(pricing: Value) -> Catalog {
        Catalog {
            fetched_at: 1,
            data: vec![Model {
                id: "vendor/model".into(),
                pricing,
            }],
        }
    }

    #[test]
    fn exact_and_unique_model_resolution_preserves_variants_and_rejects_bad_prices() {
        let mut c = fixture(
            json!({"prompt":"0.000002","completion":"0.000008","input_cache_read":"0.0000005"}),
        );
        let q = c.quote("model").unwrap();
        assert_eq!(q.price.model, "vendor/model");
        assert_eq!(q.price.input, 2.0);
        assert_eq!(q.price.cache_write, 2.0);
        assert_eq!(q.price.cache_read, 0.5);
        assert!(c.quote("model:free").is_none());
        c.data.push(Model {
            id: "other/model".into(),
            pricing: c.data[0].pricing.clone(),
        });
        assert!(c.quote("model").is_none());
        assert!(c.quote("vendor/model").is_some());
        for price in [json!("NaN"), json!("-1"), json!("infinity"), json!({})] {
            assert!(
                fixture(json!({"prompt":price,"completion":"0.1"}))
                    .quote("model")
                    .is_none()
            );
        }
        assert!(
            fixture(json!({"prompt":"0","completion":"0"}))
                .quote("model")
                .is_some()
        );
        let mut router = fixture(json!({"prompt":"0","completion":"0"}));
        router.data[0].id = "openrouter/auto".into();
        assert!(router.quote("auto").is_none());
        assert!(router.quote("openrouter/auto").is_none());
    }

    #[test]
    fn conditional_rates_follow_context_threshold_order_and_utc_windows() {
        let c = fixture(
            json!({"prompt":"0.000001","completion":"0.000004","overrides":[
                {"min_prompt_tokens":100,"prompt":"0.000002","completion":"0.000008"},
                {"utc_start":2300,"utc_end":100,"utc_days":["mon"],"completion":"0.000003"}
            ]}),
        );
        let q = c.quote("model").unwrap();
        let monday = time::OffsetDateTime::from_unix_timestamp(4 * 86400 + 23 * 3600).unwrap();
        let at_threshold = Usage {
            input: 100,
            output: 10,
            ..Usage::default()
        };
        assert!((q.cost_at(&at_threshold, monday) - 0.00013).abs() < 1e-12);
        let above = Usage {
            input: 50,
            cache_read: 51,
            output: 10,
            ..Usage::default()
        };
        assert!((q.cost_at(&above, monday) - 0.000181).abs() < 1e-12);
        let tuesday = monday + time::Duration::hours(2);
        assert!((q.cost_at(&above, tuesday) - 0.000231).abs() < 1e-12);
        assert!(fixture(json!({"prompt":"0","completion":"0","overrides":[{"utc_start":2600,"utc_end":100}]})).quote("model").is_none());
        assert!(fixture(json!({"prompt":"0","completion":"0","overrides":[{"unknown_condition":100,"prompt":"0.5"}]})).quote("model").is_none());
        assert!(
            fixture(json!({"prompt":"0","completion":"0","overrides":{}}))
                .quote("model")
                .is_none()
        );
    }

    #[test]
    fn bundled_prices_cover_real_slugs_and_long_context() {
        let c = Catalog::bundled();
        assert!(c.known().len() > 100);
        assert_eq!(
            c.quote("claude-opus-5-5").unwrap().price.model,
            "anthropic/claude-opus-5.5"
        );
        assert_eq!(
            c.quote("claude-sonnet-5-5").unwrap().price.model,
            "anthropic/claude-sonnet-5.5"
        );
        let q = c.quote("glm-5.3-flash").unwrap();
        assert!(
            (q.cost(&Usage {
                input: 1_000_000,
                output: 1_000_000,
                ..Usage::default()
            }) - 0.65)
                .abs()
                < 1e-12
        );
        let q = c.quote("openai/gpt-5.5").unwrap();
        assert!(
            (q.cost(&Usage {
                input: 300_000,
                output: 10_000,
                ..Usage::default()
            }) - 3.45)
                .abs()
                < 1e-12
        );
    }

    #[test]
    fn cache_writes_are_disjoint_in_both_openrouter_dialects() {
        let chat =
            crate::chat::usage_from_chat(&json!({"prompt_tokens":1000,"completion_tokens":20,
            "prompt_tokens_details":{"cached_tokens":300,"cache_write_tokens":600}}));
        let responses =
            crate::responses::usage_from_responses(&json!({"input_tokens":1000,"output_tokens":20,
            "input_tokens_details":{"cached_tokens":300,"cache_write_tokens":600}}));
        assert_eq!(chat, responses);
        assert_eq!(
            (
                chat.input,
                chat.cache_read,
                chat.cache_write,
                chat.total_input()
            ),
            (100, 300, 600, 1000)
        );
    }
}
