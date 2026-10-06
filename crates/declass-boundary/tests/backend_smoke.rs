// SPDX-License-Identifier: GPL-3.0-or-later
//! Live smoke tests, one per local backend: a tiny digest and a tiny answer
//! through `LocalReader`. Ignored by default and skipped unless the backend's
//! URL is set, e.g.
//!
//! ```sh
//! DECLASS_LIVE_OLLAMA_URL=http://127.0.0.1:11434/v1 DECLASS_LIVE_OLLAMA_MODEL=qwen3:8b \
//!     cargo test -p declass-boundary --test backend_smoke -- --ignored
//! ```
//!
//! Variables: `DECLASS_LIVE_<BACKEND>_URL` (required) and `DECLASS_LIVE_<BACKEND>_MODEL`
//! (default: the first model the server lists) for OLLAMA, LMSTUDIO, LLAMACPP,
//! VLLM, OMLX and MLX. The endpoint must be loopback (tunnel a remote server).

use declass_boundary::local::LocalReader;
use declass_provider::backends::{list_models, model_ids};
use declass_provider::{ChatProvider, ProviderConfig, Role};
use std::time::Duration;

const LOG: &str = "2026-09-24 10:00:01 INFO worker started\n\
2026-09-24 10:00:02 ERROR payment gateway timed out after 30s\n\
2026-09-24 10:00:03 INFO retry succeeded\n";

async fn smoke(backend: &str) {
    let var = format!("DECLASS_LIVE_{backend}_URL");
    let Ok(url) = std::env::var(&var) else {
        eprintln!("{var} is not set; skipped");
        return;
    };
    let endpoint = declass_provider::endpoint::ApprovedEndpoint::new(
        &url,
        &Role::Local {
            allowlist: Vec::new(),
            allow_plaintext: false,
        },
    )
    .expect("loopback endpoint");
    let model = match std::env::var(format!("DECLASS_LIVE_{backend}_MODEL")) {
        Ok(m) => m,
        Err(_) => {
            let listing = list_models(&endpoint, None, Duration::from_secs(10))
                .await
                .unwrap_or_else(|e| panic!("{url}: {e}"));
            model_ids(&listing)
                .into_iter()
                .next()
                .unwrap_or_else(|| panic!("{url} lists no model"))
        }
    };
    let mut pc = ProviderConfig::new(
        &url,
        &model,
        Role::Local {
            allowlist: Vec::new(),
            allow_plaintext: false,
        },
    );
    pc.first_byte_timeout = Duration::from_secs(300);
    pc.max_attempts = Some(2);
    let reader = LocalReader::new(ChatProvider::with_reqwest(pc).expect("loopback endpoint"));

    let digest = reader.digest("app.log", LOG).await.expect("digest");
    assert!(!digest.summary.trim().is_empty(), "{digest:?}");
    let answer = reader
        .answer("app.log", LOG, "Which component timed out?")
        .await
        .expect("answer");
    let said = answer.answer.to_lowercase();
    assert!(
        !answer.unanswerable && (said.contains("gateway") || said.contains("payment")),
        "{answer:?}"
    );
    eprintln!("{backend} {model}: {:?}", reader.take_stats());
}

macro_rules! live {
    ($($name:ident => $backend:literal),* $(,)?) => {$(
        #[tokio::test]
        #[ignore = "live: needs a running local server (DECLASS_LIVE_*_URL)"]
        async fn $name() {
            smoke($backend).await;
        }
    )*};
}

live!(
    ollama => "OLLAMA",
    lmstudio => "LMSTUDIO",
    llamacpp => "LLAMACPP",
    vllm => "VLLM",
    omlx => "OMLX",
    mlx => "MLX",
);
