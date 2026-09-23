// SPDX-License-Identifier: GPL-3.0-or-later
//! Context-window discovery for local servers.
//!
//! Order: the OpenAI-style `/models` listing (oMLX, vLLM, LM Studio), then the
//! server-native endpoints — llama.cpp `/props`, Ollama `/api/show`, LM Studio
//! `/api/v0/models`. Returns `None` when nothing reports a window; callers then
//! use configuration.

use serde_json::Value;

const CONTEXT_KEYS: [&str; 5] = [
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
pub fn context_from_native(document: &Value) -> Option<u64> {
    find_context(document)
}

/// Queries `base_url` (ending in `/v1`) for `model`'s context window.
pub async fn probe_context_window(
    base_url: &str,
    model: &str,
    bearer: Option<&str>,
) -> Option<u64> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;
    let auth = |rb: reqwest::RequestBuilder| match bearer {
        Some(k) => rb.bearer_auth(k),
        None => rb,
    };
    let base = base_url.trim_end_matches('/');
    let root = base.strip_suffix("/v1").unwrap_or(base);
    if let Ok(resp) = auth(client.get(format!("{base}/models"))).send().await
        && let Ok(v) = resp.json::<Value>().await
        && let Some(n) = context_from_models_listing(&v, model)
    {
        return Some(n);
    }
    let attempts = [
        auth(client.get(format!("{root}/props"))),
        auth(
            client
                .post(format!("{root}/api/show"))
                .json(&serde_json::json!({"model": model})),
        ),
        auth(client.get(format!("{root}/api/v0/models/{model}"))),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    }
}
