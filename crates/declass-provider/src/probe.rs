// SPDX-License-Identifier: GPL-3.0-or-later
//! Context-window discovery for local servers.
//!
//! Order: the OpenAI-style `/models` listing (oMLX, vLLM, LM Studio), then the
//! server-native endpoints — llama.cpp `/props`, Ollama `/api/show`, LM Studio
//! `/api/v0/models`. Returns `None` when nothing reports a window; callers then
//! use configuration.

use crate::client::ChatProvider;
use crate::error::ProviderError;
use crate::types::{Item, Request, UsageStatus};
use serde_json::Value;

// LM Studio reports both the loaded window and the model's maximum; the loaded one comes first.
const CONTEXT_KEYS: [&str; 6] = [
    "loaded_context_length",
    "max_model_len",
    "context_length",
    "max_context_length",
    "context_window",
    "n_ctx",
];

fn find_context(v: &Value) -> Option<u64> {
    match v {
        Value::Object(map) => {
            for key in CONTEXT_KEYS {
                if let Some(n) = map.get(key).and_then(Value::as_u64) {
                    return Some(n);
                }
            }
            // Ollama reports `<arch>.context_length` inside model_info.
            if let Some(n) = map
                .iter()
                .find(|(k, _)| k.ends_with(".context_length"))
                .and_then(|(_, v)| v.as_u64())
            {
                return Some(n);
            }
            map.values().find_map(find_context)
        }
        Value::Array(items) => items.iter().find_map(find_context),
        _ => None,
    }
}

/// Context window for `model` in an OpenAI-style `/models` listing.
pub fn context_from_models_listing(listing: &Value, model: &str) -> Option<u64> {
    listing
        .get("data")
        .and_then(Value::as_array)?
        .iter()
        .find(|m| m.get("id").and_then(Value::as_str) == Some(model))
        .and_then(find_context)
}

/// Context window in a llama.cpp `/props`, Ollama `/api/show` or LM Studio model object.
/// Ollama's served window (`num_ctx` in the model's parameters) wins over the
/// model's trained maximum when it is set.
pub fn context_from_native(document: &Value) -> Option<u64> {
    let num_ctx = document
        .get("parameters")
        .and_then(Value::as_str)
        .and_then(|p| {
            p.lines().find_map(|l| {
                let mut words = l.split_whitespace();
                (words.next() == Some("num_ctx"))
                    .then(|| words.next()?.parse().ok())
                    .flatten()
            })
        });
    num_ctx.or_else(|| find_context(document))
}

/// Queries `base_url` (ending in `/v1`) for `model`'s context window.
pub async fn probe_context_window(
    endpoint: &crate::endpoint::ApprovedEndpoint,
    model: &str,
    bearer: Option<&str>,
) -> Option<u64> {
    let client = crate::http::client(
        endpoint,
        std::time::Duration::from_secs(10),
        Some(std::time::Duration::from_secs(10)),
    )
    .ok()?;
    let auth = |rb: reqwest::RequestBuilder| match bearer {
        Some(k) => rb.bearer_auth(k),
        None => rb,
    };
    let base = endpoint.as_str();
    let root = base.strip_suffix("/v1").unwrap_or(base);
    if let Ok(resp) = auth(client.get(format!("{base}/models")).ok()?)
        .send()
        .await
        && resp.status().is_success()
        && let Ok(v) = resp.json::<Value>().await
        && let Some(n) = context_from_models_listing(&v, model)
    {
        return Some(n);
    }
    let attempts = [
        auth(client.get(format!("{root}/props")).ok()?),
        auth(
            client
                .post(format!("{root}/api/show"))
                .ok()?
                .json(&serde_json::json!({"model": model})),
        ),
        auth(client.get(format!("{root}/api/v0/models/{model}")).ok()?),
    ];
    for rb in attempts {
        if let Ok(resp) = rb.send().await
            && resp.status().is_success()
            && let Ok(v) = resp.json::<Value>().await
            && let Some(n) = context_from_native(&v)
        {
            return Some(n);
        }
    }
    None
}

/// What two identical requests showed about the server's prompt cache.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheReuse {
    /// Prompt tokens of the second request (cached and not).
    pub prompt_tokens: u64,
    pub first_cached: u64,
    pub second_cached: u64,
    pub first_seconds: f64,
    pub second_seconds: f64,
    /// The server reported no usage, so cached counts are unknown.
    pub unreported: bool,
}

impl CacheReuse {
    /// Share of the second prompt served from the cache.
    pub fn reuse(&self) -> f64 {
        if self.prompt_tokens == 0 {
            0.0
        } else {
            self.second_cached as f64 / self.prompt_tokens as f64
        }
    }
}

/// The probe's fixed prompt: long enough (about 1,500 tokens) for servers
/// that cache in blocks, with nothing from any workspace in it.
pub fn cache_probe_request() -> Request {
    let reference: String = (1..=120)
        .map(|i| format!("Reference line {i:03}: the quick brown fox jumps over the lazy dog.\n"))
        .collect();
    Request {
        system: format!("You answer with one word.\n{reference}"),
        items: vec![Item::User {
            text: "Reply with the single word: ok".into(),
        }],
        max_output_tokens: Some(8),
        temperature: Some(0.0),
        ..Request::default()
    }
}

/// The probe's fixed prompt for a frontier: a stable prefix of about 5,000
/// tokens, above every hosted provider's minimum cacheable prefix, and no
/// sampling settings (current reasoning models refuse `temperature`). Nothing
/// from any workspace is in it.
pub fn frontier_cache_probe_request() -> Request {
    let reference: String = (1..=400)
        .map(|i| format!("Reference line {i:03}: the quick brown fox jumps over the lazy dog.\n"))
        .collect();
    Request {
        system: format!("You answer with one word.\n{reference}"),
        items: vec![Item::User {
            text: "Reply with the single word: ok".into(),
        }],
        // Room for a reasoning model to think before its word.
        max_output_tokens: Some(256),
        ..Request::default()
    }
}

/// Sends the same short request twice and reports the cached prompt tokens
/// of each. Two model calls: run it only when the operator asks.
pub async fn cache_reuse(provider: &ChatProvider) -> Result<CacheReuse, ProviderError> {
    cache_reuse_with(provider, &cache_probe_request()).await
}

/// [`cache_reuse`] with a given probe request.
pub async fn cache_reuse_with(
    provider: &ChatProvider,
    request: &Request,
) -> Result<CacheReuse, ProviderError> {
    let started = std::time::Instant::now();
    let first = provider.create(request).await?;
    let first_seconds = started.elapsed().as_secs_f64();
    let started = std::time::Instant::now();
    let second = provider.create(request).await?;
    let second_seconds = started.elapsed().as_secs_f64();
    Ok(CacheReuse {
        prompt_tokens: second.usage.input + second.usage.cache_read + second.usage.cache_write,
        first_cached: first.usage.cache_read,
        second_cached: second.usage.cache_read,
        first_seconds,
        second_seconds,
        unreported: second.usage.status == UsageStatus::Estimated,
    })
}

/// The colours the vision probe shows, one image each.
const PROBE_COLOURS: [(&str, [u8; 3]); 2] = [("red", [220, 30, 30]), ("blue", [30, 60, 220])];

/// What the vision probe found.
#[derive(Debug, Clone, PartialEq)]
pub struct VisionProbe {
    /// The model named the colour of every probe image.
    pub reads_images: bool,
    /// The expected colour and the model's answer, per probe image.
    pub answers: Vec<(String, String)>,
    /// Input tokens reported for the first probe; an image costs tens to
    /// hundreds, so a handful means the server dropped it.
    pub prompt_tokens: u64,
}

/// The request of one vision probe: a built-in image of one colour (nothing
/// from a workspace) and the question which colour it is.
pub fn vision_probe_request(rgb: [u8; 3]) -> Request {
    let image = crate::image::prepare(&crate::image::solid_png(64, 64, rgb), 64)
        .expect("a generated PNG is valid");
    Request {
        system: String::new(),
        items: vec![
            Item::User {
                text: "Which single colour fills this image? Answer with one lowercase word."
                    .into(),
            },
            Item::Images {
                call_id: None,
                images: vec![image],
            },
        ],
        max_output_tokens: Some(512),
        ..Request::default()
    }
}

/// Whether the model reads images: it is shown a red and then a blue image
/// and must name each colour (and not the other). A server that drops image
/// parts answers from the text alone. `extra` is merged into each request
/// (a local server's template settings).
pub async fn vision_probe(
    provider: &ChatProvider,
    extra: &serde_json::Map<String, Value>,
) -> Result<VisionProbe, ProviderError> {
    let mut answers = Vec::new();
    let mut prompt_tokens = 0;
    let mut reads = true;
    for (i, (name, rgb)) in PROBE_COLOURS.iter().enumerate() {
        let mut req = vision_probe_request(*rgb);
        req.extra = extra.clone();
        let r = provider.create(&req).await?;
        if i == 0 {
            prompt_tokens = r.usage.input + r.usage.cache_read + r.usage.cache_write;
        }
        let words: Vec<String> = r
            .text
            .to_lowercase()
            .split(|c: char| !c.is_alphabetic())
            .filter(|w| !w.is_empty())
            .map(str::to_owned)
            .collect();
        let named = |c: &str| words.iter().any(|w| w == c);
        let others = PROBE_COLOURS
            .iter()
            .any(|(other, _)| other != name && named(other));
        reads &= named(name) && !others;
        answers.push(((*name).to_owned(), r.text.trim().chars().take(80).collect()));
    }
    Ok(VisionProbe {
        reads_images: reads,
        answers,
        prompt_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approved(server: &crate::mock_http::MockServer) -> crate::endpoint::ApprovedEndpoint {
        crate::endpoint::ApprovedEndpoint::new(
            &server.base_url(),
            &crate::Role::Local {
                allowlist: Vec::new(),
                allow_plaintext: false,
            },
        )
        .unwrap()
    }
    use serde_json::json;

    #[test]
    fn the_frontier_probe_is_cacheable_everywhere_and_sets_no_sampling() {
        let r = frontier_cache_probe_request();
        // Above the largest documented minimum cacheable prefix (4,096 tokens).
        assert!(crate::types::estimate_tokens(r.system.len()) > 4_096);
        assert!(r.temperature.is_none() && r.extra.is_empty());
        assert!(r.max_output_tokens.unwrap() >= 16);
    }

    #[test]
    fn reads_models_listing() {
        let v = json!({"data":[{"id":"a","max_model_len":8192},{"id":"omlx-coding","max_model_len":65536}]});
        assert_eq!(context_from_models_listing(&v, "omlx-coding"), Some(65536));
        assert_eq!(context_from_models_listing(&v, "missing"), None);
    }

    #[test]
    fn reads_native_documents() {
        assert_eq!(
            context_from_native(&json!({"default_generation_settings":{"n_ctx":32768}})),
            Some(32768)
        );
        assert_eq!(
            context_from_native(&json!({"model_info":{"qwen3.context_length":40960}})),
            Some(40960)
        );
        assert_eq!(
            context_from_native(&json!({"id":"m","max_context_length":131072})),
            Some(131072)
        );
        assert_eq!(
            context_from_native(
                &json!({"max_context_length":131072,"loaded_context_length":32768})
            ),
            Some(32768)
        );
        assert_eq!(
            context_from_native(&json!({
                "parameters":"stop \"<|im_end|>\"\nnum_ctx 16384",
                "model_info":{"qwen3.context_length":40960}
            })),
            Some(16384)
        );
    }

    #[tokio::test]
    async fn context_probe_ignores_redirects_and_unsuccessful_listings() {
        use crate::mock_http::MockServer;
        let body = r#"{"data":[{"id":"q","max_model_len":65536}]}"#;
        let destination = MockServer::start(&[("GET /v1/models", 200, body)]);
        let location = format!("{}/models", destination.base_url());
        let source = MockServer::start_with_headers(
            &[("GET /v1/models", 307, body)],
            &[("location", &location)],
        );
        assert_eq!(
            probe_context_window(&approved(&source), "q", Some("k")).await,
            None
        );
        assert!(destination.seen().is_empty());
        assert_eq!(source.seen().len(), 4);
        let denied = MockServer::start(&[("GET /v1/models", 403, body)]);
        assert_eq!(
            probe_context_window(&approved(&denied), "q", None).await,
            None
        );
    }

    #[tokio::test]
    async fn probes_listing_then_native_endpoints() {
        use crate::mock_http::MockServer;
        let vllm = MockServer::start(&[(
            "GET /v1/models",
            200,
            r#"{"data":[{"id":"q","max_model_len":65536}]}"#,
        )]);
        assert_eq!(
            probe_context_window(&approved(&vllm), "q", None).await,
            Some(65536)
        );
        let ollama = MockServer::start(&[
            ("GET /v1/models", 200, r#"{"data":[{"id":"q:8b"}]}"#),
            (
                "POST /api/show",
                200,
                r#"{"model_info":{"qwen3.context_length":40960}}"#,
            ),
        ]);
        assert_eq!(
            probe_context_window(&approved(&ollama), "q:8b", Some("k")).await,
            Some(40960)
        );
        assert!(ollama.seen().iter().all(|r| r.bearer));
        let silent = MockServer::start(&[("GET /v1/models", 200, r#"{"data":[{"id":"m"}]}"#)]);
        assert_eq!(
            probe_context_window(&approved(&silent), "m", None).await,
            None
        );
    }
}
