// SPDX-License-Identifier: GPL-3.0-or-later
//! Local backend presets and loopback discovery.
//!
//! Every preset speaks OpenAI-compatible Chat Completions under `/v1`; they
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
];

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
}

fn client(timeout: Duration) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(timeout.min(Duration::from_millis(500)))
        .timeout(timeout)
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
    let mut rb = client.get(url);
    if let Some(k) = bearer {
        rb = rb.bearer_auth(k);
    }
    let resp = rb.send().await.map_err(|e| {
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
    resp.json::<Value>()
        .await
        .map_err(|_| "reply is not JSON".to_owned())
}

/// Model ids in an OpenAI-style `/models` listing.
pub fn model_ids(listing: &Value) -> Vec<String> {
    listing
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
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
        let listing = list_models(&base_url, None, timeout).await.ok()?;
        let backend = identify(&base_url, &listing, None, timeout).await;
        Some(Server {
            models: model_ids(&listing),
            base_url,
            backend,
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
        assert_eq!(loopback_ports(), vec![11434, 1234, 8080, 8000]);
    }

    fn server(models: &[&str]) -> Server {
        Server {
            base_url: "http://127.0.0.1:1/v1".into(),
            backend: None,
            models: models.iter().map(|m| (*m).to_owned()).collect(),
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
