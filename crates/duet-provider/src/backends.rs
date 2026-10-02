// SPDX-License-Identifier: GPL-3.0-or-later
//! Local backend presets and loopback discovery, and the frontier presets.
//!
//! Every local preset speaks OpenAI-compatible Chat Completions under `/v1`; they
//! differ in default port and in how they report models and context length.
//! Discovery only ever contacts `127.0.0.1` on the preset ports: it never scans
//! other hosts. It lists models (`GET /v1/models`) and tells servers apart by
//! their documented native endpoints; it never calls a model.

use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Preset {
    /// The name `duet config preset` takes.
    pub name: &'static str,
    pub label: &'static str,
    pub port: u16,
    /// Path of the OpenAI-compatible API on the server.
    pub base_path: &'static str,
    /// Where the server lists its models.
    pub models: &'static str,
    /// Where the server reports a model's context window.
    pub context: &'static str,
    /// A model id of the form the server expects (shown in hints only).
    pub example_model: &'static str,
}

impl Preset {
    /// The loopback `local.base_url` for this backend.
    pub fn base_url(&self, port: Option<u16>) -> String {
        format!(
            "http://127.0.0.1:{}{}",
            port.unwrap_or(self.port),
            self.base_path
        )
    }
}

pub const PRESETS: &[Preset] = &[
    Preset {
        name: "ollama",
        label: "Ollama",
        port: 11434,
        base_path: "/v1",
        models: "GET /v1/models; native GET /api/tags",
        context: "POST /api/show: num_ctx in parameters when set, else model_info.<arch>.context_length (the model's maximum; the served window is num_ctx or OLLAMA_CONTEXT_LENGTH)",
        example_model: "qwen3-coder:30b",
    },
    Preset {
        name: "lmstudio",
        label: "LM Studio",
        port: 1234,
        base_path: "/v1",
        models: "GET /v1/models; native GET /api/v0/models",
        context: "GET /api/v0/models/<id>: loaded_context_length when loaded, else max_context_length",
        example_model: "qwen/qwen3-coder-30b",
    },
    Preset {
        name: "llamacpp",
        label: "llama.cpp server",
        port: 8080,
        base_path: "/v1",
        models: "GET /v1/models (the one loaded model)",
        context: "GET /props: default_generation_settings.n_ctx (the served window)",
        example_model: "the alias given with --alias, or the model file name",
    },
    Preset {
        name: "vllm",
        label: "vLLM",
        port: 8000,
        base_path: "/v1",
        models: "GET /v1/models",
        context: "GET /v1/models: max_model_len",
        example_model: "Qwen/Qwen3-Coder-30B-A3B-Instruct",
    },
    Preset {
        name: "omlx",
        label: "oMLX",
        port: 8000,
        base_path: "/v1",
        models: "GET /v1/models",
        context: "GET /v1/models: max_model_len or context_length when reported, else context.window_tokens",
        example_model: "the model directory name, e.g. omlx-coding",
    },
    Preset {
        name: "mlx",
        label: "MLX (mlx_lm.server)",
        port: 8080,
        base_path: "/v1",
        models: "GET /v1/models",
        context: "not reported over HTTP",
        example_model: "mlx-community/Qwen3-Coder-30B-A3B-Instruct-4bit",
    },
    Preset {
        name: "jan",
        label: "Jan local API",
        port: 1337,
        base_path: "/v1",
        models: "GET /v1/models (requires Jan's local API key)",
        context: "check the model's loaded context in Jan",
        example_model: "the model id shown by Jan",
    },
    Preset {
        name: "gpt4all",
        label: "GPT4All",
        port: 4891,
        base_path: "/v1",
        models: "GET /v1/models",
        context: "check the model's context in GPT4All",
        example_model: "the model name shown by GPT4All",
    },
    Preset {
        name: "koboldcpp",
        label: "KoboldCpp",
        port: 5001,
        base_path: "/v1",
        models: "GET /v1/models",
        context: "GET /api/extra/true_max_context_length",
        example_model: "the loaded model id",
    },
    Preset {
        name: "localai",
        label: "LocalAI",
        port: 8080,
        base_path: "/v1",
        models: "GET /v1/models",
        context: "check the served model's context in LocalAI",
        example_model: "the served model id",
    },
    Preset {
        name: "litellm",
        label: "LiteLLM proxy",
        port: 4000,
        base_path: "/v1",
        models: "GET /v1/models (may require a proxy key)",
        context: "depends on the model behind the proxy",
        example_model: "the configured model alias",
    },
];

/// A known frontier provider: endpoint, a default model, the key variable and
/// the dialect its endpoint speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FrontierPreset {
    /// The name `duet config preset` takes.
    pub name: &'static str,
    pub label: &'static str,
    pub base_url: &'static str,
    pub model: &'static str,
    pub api_key_env: &'static str,
    /// A `frontier.dialect` value.
    pub dialect: &'static str,
    /// Whether the preset's default model accepts images (`frontier.vision`).
    pub vision: bool,
}

pub const FRONTIER_PRESETS: &[FrontierPreset] = &[
    FrontierPreset {
        name: "zai",
        label: "z.ai GLM (coding plan)",
        base_url: "https://api.z.ai/api/coding/paas/v4",
        model: "glm-5.3-flash",
        api_key_env: "ZAI_API_KEY",
        dialect: "chat",
        vision: false,
    },
    FrontierPreset {
        name: "anthropic",
        label: "Anthropic (Messages API)",
        base_url: "https://api.anthropic.com/v1",
        model: "claude-sonnet-5-5",
        api_key_env: "ANTHROPIC_API_KEY",
        dialect: "anthropic",
        vision: true,
    },
    FrontierPreset {
        name: "openai",
        label: "OpenAI (Responses API)",
        base_url: "https://api.openai.com/v1",
        model: "gpt-6.1-sol",
        api_key_env: "OPENAI_API_KEY",
        dialect: "responses",
        vision: true,
    },
    FrontierPreset {
        name: "gemini",
        label: "Google Gemini",
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        model: "gemini-3.8-flash",
        api_key_env: "GEMINI_API_KEY",
        dialect: "chat",
        vision: true,
    },
    FrontierPreset {
        name: "openrouter",
        label: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        model: "z-ai/glm-5.3-flash",
        api_key_env: "OPENROUTER_API_KEY",
        dialect: "chat",
        vision: false,
    },
    FrontierPreset {
        name: "deepseek",
        label: "DeepSeek",
        base_url: "https://api.deepseek.com",
        model: "deepseek-v4-pro",
        api_key_env: "DEEPSEEK_API_KEY",
        dialect: "chat",
        vision: false,
    },
    FrontierPreset {
        name: "xai",
        label: "xAI",
        base_url: "https://api.x.ai/v1",
        model: "grok-4.7",
        api_key_env: "XAI_API_KEY",
        dialect: "chat",
        vision: true,
    },
    FrontierPreset {
        name: "mistral",
        label: "Mistral AI",
        base_url: "https://api.mistral.ai/v1",
        model: "",
        api_key_env: "MISTRAL_API_KEY",
        dialect: "chat",
        vision: false,
    },
    FrontierPreset {
        name: "groq",
        label: "GroqCloud",
        base_url: "https://api.groq.com/openai/v1",
        model: "openai/gpt-oss-120b",
        api_key_env: "GROQ_API_KEY",
        dialect: "chat",
        vision: false,
    },
    FrontierPreset {
        name: "cerebras",
        label: "Cerebras Inference",
        base_url: "https://api.cerebras.ai/v1",
        model: "gpt-oss-120b",
        api_key_env: "CEREBRAS_API_KEY",
        dialect: "chat",
        vision: false,
    },
    FrontierPreset {
        name: "together",
        label: "Together AI",
        base_url: "https://api.together.ai/v1",
        model: "",
        api_key_env: "TOGETHER_API_KEY",
        dialect: "chat",
        vision: false,
    },
    FrontierPreset {
        name: "fireworks",
        label: "Fireworks AI",
        base_url: "https://api.fireworks.ai/inference/v1",
        model: "",
        api_key_env: "FIREWORKS_API_KEY",
        dialect: "chat",
        vision: false,
    },
    FrontierPreset {
        name: "qwen",
        label: "Alibaba Qwen (Singapore)",
        base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        model: "qwen-plus",
        api_key_env: "DASHSCOPE_API_KEY",
        dialect: "chat",
        vision: false,
    },
];

pub fn frontier_preset(name: &str) -> Option<&'static FrontierPreset> {
    FRONTIER_PRESETS
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
}

pub fn preset(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

/// The distinct preset ports, in preset order.
pub fn loopback_ports() -> Vec<u16> {
    let mut ports = Vec::new();
    for p in PRESETS {
        if !ports.contains(&p.port) {
            ports.push(p.port);
        }
    }
    // Jan CLI serves on 6767; Jan Desktop commonly uses 1337.
    ports.push(6767);
    ports
}

/// A server that answered on a loopback port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Server {
    pub base_url: String,
    /// The identified backend (preset label), or `None` for a generic
    /// OpenAI-compatible server (oMLX, mlx_lm.server and others look alike).
    pub backend: Option<&'static str>,
    pub models: Vec<String>,
    /// Environment variable used for discovery and later model requests.
    pub api_key_env: Option<&'static str>,
}

const MAX_LISTING_BYTES: usize = 4 * 1024 * 1024;

fn client(timeout: Duration) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(timeout.min(Duration::from_millis(500)))
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()
}

fn root(base_url: &str) -> &str {
    let base = base_url.trim_end_matches('/');
    base.strip_suffix("/v1").unwrap_or(base)
}

async fn get_json(
    client: &reqwest::Client,
    url: &str,
    bearer: Option<&str>,
) -> Result<Value, String> {
    let headers = bearer
        .map(|k| crate::Dialect::Chat.auth_headers(k))
        .unwrap_or_default();
    get_json_with(client, url, &headers).await
}

async fn get_json_with(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
) -> Result<Value, String> {
    let mut rb = client.get(url);
    for (k, v) in headers {
        rb = rb.header(k, v);
    }
    let mut resp = rb.send().await.map_err(|e| {
        if e.is_connect() {
            "connection refused".to_owned()
        } else if e.is_timeout() {
            "timed out".to_owned()
        } else {
            e.to_string()
        }
    })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    if resp
        .content_length()
        .is_some_and(|n| n > MAX_LISTING_BYTES as u64)
    {
        return Err("model listing exceeds size limit".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|_| "model listing read failed")?
    {
        if body.len().saturating_add(chunk.len()) > MAX_LISTING_BYTES {
            return Err("model listing exceeds size limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| "reply is not JSON".to_owned())
}

/// Model ids in an OpenAI-style `/models` listing.
pub fn model_ids(listing: &Value) -> Vec<String> {
    listing
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| listing.as_array())
        .into_iter()
        .flatten()
        .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

/// Models suitable for Duet's tool-driven text work, when a provider exposes
/// capability metadata. Unknown capabilities are left for an explicit choice
/// and `doctor --online` rather than guessed from a marketing label.
pub fn agent_model_ids(listing: &Value) -> Vec<String> {
    let entries = listing
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| listing.get("models").and_then(Value::as_array))
        .or_else(|| listing.as_array());
    let Some(entries) = entries else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .iter()
        .filter(|m| {
            let caps = m.get("capabilities");
            !matches!(
                caps.and_then(|c| c.get("completion_chat"))
                    .and_then(Value::as_bool),
                Some(false)
            ) && !matches!(
                caps.and_then(|c| c.get("function_calling"))
                    .and_then(Value::as_bool),
                Some(false)
            ) && !matches!(
                m.get("supports_tool_calls").and_then(Value::as_bool),
                Some(false)
            )
        })
        .filter_map(|m| {
            m.get("id")
                .or_else(|| m.get("name"))
                .and_then(Value::as_str)
        })
        .filter(|id| id.len() <= 200 && !id.chars().any(char::is_control))
        .filter(|id| {
            let id = id.to_ascii_lowercase();
            ![
                "embedding",
                "embed",
                "rerank",
                "whisper",
                "transcrib",
                "tts",
                "moderation",
                "guard",
                "image-generation",
                "flux",
                "diffusion",
                "video",
                "realtime",
                "audio",
            ]
            .iter()
            .any(|word| id.contains(word))
        })
        .map(str::to_owned)
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// A model listing can advertise image input under several common schemas.
/// An explicit false overrides a preset's default capability claim.
pub fn model_vision_capability(listing: &Value, model: &str) -> Option<bool> {
    let entry = listing
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| listing.as_array())
        .into_iter()
        .flatten()
        .find(|m| m.get("id").and_then(Value::as_str) == Some(model));
    let entry = entry?;
    if let Some(vision) = entry
        .get("capabilities")
        .and_then(|v| v.get("vision"))
        .and_then(Value::as_bool)
    {
        return Some(vision);
    }
    let modalities = entry
        .get("architecture")
        .and_then(|v| v.get("input_modalities"))
        .or_else(|| entry.get("input_modalities"));
    modalities
        .and_then(Value::as_array)
        .map(|list| list.iter().any(|v| v.as_str() == Some("image")))
}

/// The models `base_url` lists (`GET <base_url>/models`), as the raw listing.
pub async fn list_models(
    base_url: &str,
    bearer: Option<&str>,
    timeout: Duration,
) -> Result<Value, String> {
    let client = client(timeout).ok_or("cannot build an HTTP client")?;
    get_json(
        &client,
        &format!("{}/models", base_url.trim_end_matches('/')),
        bearer,
    )
    .await
}

/// The models a frontier endpoint lists (`GET <base_url>/models`), with the
/// credential headers of its dialect.
pub async fn list_frontier_models(
    base_url: &str,
    dialect: crate::Dialect,
    key: Option<&str>,
    timeout: Duration,
) -> Result<Value, String> {
    let client = client(timeout).ok_or("cannot build an HTTP client")?;
    let mut headers = dialect.fixed_headers();
    if let Some(k) = key {
        headers.extend(dialect.auth_headers(k));
    }
    let result = get_json_with(
        &client,
        &format!("{}/models", base_url.trim_end_matches('/')),
        &headers,
    )
    .await;
    // Fireworks publishes its model catalog through an account-scoped API,
    // separate from its OpenAI-compatible inference path.
    if base_url == "https://api.fireworks.ai/inference/v1"
        && result
            .as_ref()
            .is_err_and(|e| e != "HTTP 401" && e != "HTTP 403")
    {
        return get_json_with(
            &client,
            "https://api.fireworks.ai/v1/accounts/fireworks/models",
            &headers,
        )
        .await;
    }
    result
}

/// Which backend serves `base_url`, from its documented native endpoints
/// (and the `/models` listing already fetched). `None`: a generic server.
pub async fn identify(
    base_url: &str,
    listing: &Value,
    bearer: Option<&str>,
    timeout: Duration,
) -> Option<&'static str> {
    let label = |name| preset(name).map(|p| p.label);
    let owned_by_vllm = listing
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|m| m.get("owned_by").and_then(Value::as_str) == Some("vllm"));
    if owned_by_vllm {
        return label("vllm");
    }
    let client = client(timeout)?;
    let root = root(base_url);
    if let Ok(v) = get_json(&client, &format!("{root}/api/version"), bearer).await
        && v.get("version").is_some()
    {
        return label("ollama");
    }
    if let Ok(v) = get_json(&client, &format!("{root}/api/v0/models"), bearer).await
        && v.get("data").is_some()
    {
        return label("lmstudio");
    }
    if let Ok(v) = get_json(&client, &format!("{root}/props"), bearer).await
        && v.get("default_generation_settings").is_some()
    {
        return label("llamacpp");
    }
    None
}

/// Probes `127.0.0.1` on each of `ports` (short timeouts) and returns the
/// servers that list models. Nothing but loopback is ever contacted.
pub async fn discover_loopback(ports: &[u16], timeout: Duration) -> Vec<Server> {
    let probes = ports.iter().map(|port| async move {
        let base_url = format!("http://127.0.0.1:{port}/v1");
        let api_key_env = match port {
            1337 | 6767 => Some("JAN_API_KEY"),
            4000 => Some("LITELLM_API_KEY"),
            _ => None,
        };
        let key = api_key_env
            .and_then(|name| std::env::var(name).ok())
            .filter(|s| !s.is_empty());
        let listing = list_models(&base_url, key.as_deref(), timeout).await.ok()?;
        let backend = identify(&base_url, &listing, key.as_deref(), timeout).await;
        Some(Server {
            models: agent_model_ids(&listing),
            base_url,
            backend,
            api_key_env: key.map(|_| api_key_env.unwrap()),
        })
    });
    futures_util::future::join_all(probes)
        .await
        .into_iter()
        .flatten()
        .collect()
}

/// What bootstrap may do with the discovered servers.
#[derive(Debug, PartialEq, Eq)]
pub enum Pick<'a> {
    /// Exactly one server and one model qualify: use them for this run.
    One { server: &'a Server, model: String },
    /// More than one choice: the owner must pick.
    Ambiguous,
    /// No server lists a model.
    Nothing,
}

/// Picks a server and model only when the choice is unambiguous: a single
/// server listing a single model, or `preferred` (a configured `local.model`)
/// listed by exactly one server.
pub fn pick<'a>(servers: &'a [Server], preferred: Option<&str>) -> Pick<'a> {
    let serving: Vec<&Server> = servers.iter().filter(|s| !s.models.is_empty()).collect();
    if let Some(want) = preferred {
        let having: Vec<&&Server> = serving
            .iter()
            .filter(|s| s.models.iter().any(|m| m == want))
            .collect();
        if let [only] = having.as_slice() {
            return Pick::One {
                server: only,
                model: want.to_owned(),
            };
        }
    }
    match serving.as_slice() {
        [] => Pick::Nothing,
        [only] if only.models.len() == 1 => Pick::One {
            server: only,
            model: only.models[0].clone(),
        },
        _ => Pick::Ambiguous,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock_http::MockServer;

    const T: Duration = Duration::from_secs(2);

    #[test]
    fn frontier_presets_use_tls_known_dialects_and_distinct_names() {
        let mut names: Vec<_> = FRONTIER_PRESETS.iter().map(|p| p.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), FRONTIER_PRESETS.len());
        assert!(FRONTIER_PRESETS.len() >= 12);
        for p in FRONTIER_PRESETS {
            assert!(p.base_url.starts_with("https://"), "{}", p.name);
            assert!(crate::Dialect::parse(p.dialect).is_some(), "{}", p.name);
            assert!(
                preset(p.name).is_none(),
                "{} is also a local preset",
                p.name
            );
            assert_eq!(frontier_preset(&p.name.to_uppercase()), Some(p));
        }
        assert_eq!(frontier_preset("anthropic").unwrap().dialect, "anthropic");
        assert_eq!(frontier_preset("openai").unwrap().dialect, "responses");
        assert_eq!(frontier_preset("zai").unwrap().dialect, "chat");
        // GLM text models take no images; Claude and GPT models do.
        assert!(!frontier_preset("zai").unwrap().vision);
        assert!(frontier_preset("anthropic").unwrap().vision);
        assert!(frontier_preset("openai").unwrap().vision);
    }

    #[test]
    fn presets_are_loopback_and_named_uniquely() {
        let mut names: Vec<_> = PRESETS.iter().map(|p| p.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PRESETS.len());
        for p in PRESETS {
            let url = p.base_url(None);
            assert!(
                crate::endpoint::check_local_endpoint(&url, &[], false).is_ok(),
                "{url}"
            );
        }
        assert_eq!(
            preset("Ollama").unwrap().base_url(None),
            "http://127.0.0.1:11434/v1"
        );
        assert_eq!(
            preset("lmstudio").unwrap().base_url(Some(4321)),
            "http://127.0.0.1:4321/v1"
        );
        assert_eq!(
            loopback_ports(),
            vec![11434, 1234, 8080, 8000, 1337, 4891, 5001, 4000, 6767]
        );
    }

    fn server(models: &[&str]) -> Server {
        Server {
            base_url: "http://127.0.0.1:1/v1".into(),
            backend: None,
            models: models.iter().map(|m| (*m).to_owned()).collect(),
            api_key_env: None,
        }
    }

    #[test]
    fn picks_only_unambiguous_choices() {
        assert_eq!(pick(&[], None), Pick::Nothing);
        assert_eq!(pick(&[server(&[])], None), Pick::Nothing);
        let one = [server(&["m"])];
        assert!(matches!(pick(&one, None), Pick::One { model, .. } if model == "m"));
        assert_eq!(pick(&[server(&["a", "b"])], None), Pick::Ambiguous);
        assert_eq!(
            pick(&[server(&["a"]), server(&["b"])], None),
            Pick::Ambiguous
        );
        // A configured model resolves it when exactly one server lists it.
        assert!(matches!(
            pick(&[server(&["a"]), server(&["b"])], Some("b")),
            Pick::One { model, .. } if model == "b"
        ));
        assert_eq!(
            pick(&[server(&["b"]), server(&["b"])], Some("b")),
            Pick::Ambiguous
        );
    }

    #[test]
    fn discovery_keeps_only_text_models_with_tool_use_when_known() {
        let listing = serde_json::json!({"data": [
            {"id":"coder", "capabilities":{"completion_chat":true,"function_calling":true}},
            {"id":"text-only", "capabilities":{"completion_chat":true,"function_calling":false}},
            {"id":"embed-large"}, {"id":"audio-transcription"},
            {"id":"bad\nmodel"}, {"id":"coder"}
        ]});
        assert_eq!(agent_model_ids(&listing), vec!["coder"]);
        assert_eq!(model_ids(&serde_json::json!([{"id":"one"}])), vec!["one"]);
        assert_eq!(
            model_vision_capability(
                &serde_json::json!({"data": [{"id":"vision","architecture":{"input_modalities":["text","image"]}}]}),
                "vision"
            ),
            Some(true)
        );
        assert_eq!(model_vision_capability(&listing, "coder"), None);
        assert_eq!(
            model_vision_capability(
                &serde_json::json!({"data":[{"id":"text","capabilities":{"vision":false}}]}),
                "text"
            ),
            Some(false)
        );
        assert_eq!(
            agent_model_ids(&serde_json::json!({"models": [
                {"name":"accounts/fireworks/models/qwen3-coder"},
                {"name":"accounts/fireworks/models/flux-kontext"}
            ]})),
            ["accounts/fireworks/models/qwen3-coder"]
        );
    }

    #[tokio::test]
    async fn local_discovery_reads_models_without_a_generation_call() {
        let server = MockServer::start(&[(
            "GET /v1/models",
            200,
            r#"{"data":[{"id":"qwen-coder"},{"id":"text-embedding"}]}"#,
        )]);
        let found = discover_loopback(&[server.port], T).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].models, ["qwen-coder"]);
        assert!(server.seen().iter().all(|request| request.method == "GET"));
    }

    #[tokio::test]
    async fn discovery_identifies_backends_by_native_endpoints() {
        let ollama = MockServer::start(&[
            (
                "GET /v1/models",
                200,
                r#"{"object":"list","data":[{"id":"qwen3:8b","object":"model","owned_by":"library"}]}"#,
            ),
            ("GET /api/version", 200, r#"{"version":"0.9.0"}"#),
        ]);
        let llama = MockServer::start(&[
            ("GET /v1/models", 200, r#"{"data":[{"id":"coder.gguf"}]}"#),
            (
                "GET /props",
                200,
                r#"{"default_generation_settings":{"n_ctx":32768}}"#,
            ),
        ]);
        let vllm = MockServer::start(&[(
            "GET /v1/models",
            200,
            r#"{"data":[{"id":"Q","owned_by":"vllm","max_model_len":65536}]}"#,
        )]);
        let lms = MockServer::start(&[
            ("GET /v1/models", 200, r#"{"data":[{"id":"a"},{"id":"b"}]}"#),
            ("GET /api/v0/models", 200, r#"{"data":[]}"#),
        ]);
        let generic = MockServer::start(&[("GET /v1/models", 200, r#"{"data":[{"id":"x"}]}"#)]);
        let broken = MockServer::start(&[("GET /v1/models", 500, "{}")]);
        // A port nothing listens on: bind, note it, release it.
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let ports = [
            ollama.port,
            llama.port,
            vllm.port,
            lms.port,
            generic.port,
            broken.port,
            closed,
        ];
        let found = discover_loopback(&ports, T).await;
        let backends: Vec<_> = found.iter().map(|s| s.backend).collect();
        assert_eq!(
            backends,
            vec![
                Some("Ollama"),
                Some("llama.cpp server"),
                Some("vLLM"),
                Some("LM Studio"),
                None
            ]
        );
        assert_eq!(found[0].models, vec!["qwen3:8b"]);
        assert_eq!(found[3].models, vec!["a", "b"]);
        assert_eq!(pick(&found, None), Pick::Ambiguous);
        // Discovery only reads listings and native metadata.
        for s in [&ollama, &llama, &vllm, &lms, &generic] {
            assert!(
                s.seen()
                    .iter()
                    .all(|r| r.method == "GET" && !r.path.contains("chat")),
                "{:?}",
                s.seen()
            );
        }
    }
}
