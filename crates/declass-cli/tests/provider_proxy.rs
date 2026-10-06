// SPDX-License-Identifier: GPL-3.0-or-later
//! Model requests and discovery must not inherit environment proxies. Each
//! case runs in a subprocess so parallel tests never mutate shared environment.

use declass_provider::client::{ReqwestTransport, Transport};
use declass_provider::mock_http::MockServer;
use std::process::Command;
use std::time::Duration;

const CHILD_ENDPOINT: &str = "DECLASS_TEST_PROVIDER_PROXY_ENDPOINT";
const PROXIES: &[&str] = &[
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
];

#[test]
fn model_requests_and_discovery_ignore_environment_proxies() {
    if let Ok(endpoint) = std::env::var(CHILD_ENDPOINT) {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let approved = declass_provider::endpoint::ApprovedEndpoint::new(
                &endpoint,
                &declass_provider::Role::Local {
                    allowlist: Vec::new(),
                    allow_plaintext: false,
                },
            )
            .unwrap();
            let transport = ReqwestTransport::new(approved.clone(), Duration::from_millis(500));
            let response = transport
                .post(
                    format!("{endpoint}/chat/completions"),
                    vec![("authorization".into(), "Bearer fictional-local-key".into())],
                    b"fictional-private-prompt".to_vec(),
                )
                .await;
            let listing = declass_provider::backends::list_models(
                &approved,
                Some("fictional-discovery-key"),
                Duration::from_millis(500),
            )
            .await;
            if endpoint.starts_with("http://") {
                assert_eq!(response.unwrap().status, 200);
                assert!(listing.unwrap()["data"].is_array());
                let context = declass_provider::probe::probe_context_window(
                    &approved,
                    "coder",
                    Some("fictional-context-key"),
                )
                .await;
                assert_eq!(context, Some(65536));
            } else {
                // This reserved loopback port has no TLS server. A refused
                // direct connection must not become a proxy CONNECT request.
                assert!(response.is_err());
                assert!(listing.is_err());
            }
        });
        return;
    }

    let direct = MockServer::start(&[
        ("POST /v1/chat/completions", 200, "{}"),
        (
            "GET /v1/models",
            200,
            r#"{"data":[{"id":"coder","max_model_len":65536}]}"#,
        ),
    ]);
    // Keep the port reserved without accepting requests, so HTTPS reliably
    // fails within the client's timeout without involving an external host.
    let tls_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let https = format!(
        "https://127.0.0.1:{}/v1",
        tls_port.local_addr().unwrap().port()
    );
    let proxy = MockServer::start(&[]);
    let proxy_url = format!("http://127.0.0.1:{}", proxy.port);
    for proxy_name in PROXIES {
        for endpoint in [direct.base_url(), https.clone()] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args([
                "--exact",
                "model_requests_and_discovery_ignore_environment_proxies",
                "--nocapture",
            ]);
            for name in PROXIES {
                command.env_remove(name);
            }
            let output = command
                .env(proxy_name, &proxy_url)
                .env("NO_PROXY", "")
                .env("no_proxy", "")
                .env(CHILD_ENDPOINT, &endpoint)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{proxy_name} / {endpoint}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(proxy.seen().is_empty(), "{proxy_name}: {:?}", proxy.seen());
        }
    }
    let seen = direct.seen();
    assert_eq!(seen.len(), PROXIES.len() * 3);
    assert!(seen.iter().all(|request| request.bearer));
}
