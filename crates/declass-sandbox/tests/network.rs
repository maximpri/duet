// SPDX-License-Identifier: GPL-3.0-or-later
//! Network modes against the real platform sandbox: a command with the proxy
//! route reaches the proxy and the servers it starts itself, and nothing else;
//! without network it reaches nothing. The proxy here is a stand-in that
//! answers every request; the real one (`declass-egress`) is tested on its own.
//! Run on Linux with `tools/linux-check.sh`.
#![cfg(any(target_os = "macos", target_os = "linux"))]

use declass_sandbox::{Network, Output, ProxyRoute, SandboxKind, Spec, bridge};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const KIND: SandboxKind = if cfg!(target_os = "linux") {
    SandboxKind::Bubblewrap
} else {
    SandboxKind::Seatbelt
};

fn helper() -> Vec<std::ffi::OsString> {
    vec![env!("CARGO_BIN_EXE_declass-sandbox-bridge").into()]
}

fn spec(ws: &Path, network: Network) -> Spec {
    Spec {
        workspace: ws.to_path_buf(),
        scratch: ws.parent().unwrap().join("scratch"),
        network,
        timeout: Duration::from_secs(60),
        output_cap: 64 * 1024,
        spill_file: None,
        extra_env: vec![],
        deny_read: Vec::new(),
        read_only: false,
    }
}

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let ws = d.path().canonicalize().unwrap().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    (d, ws)
}

async fn sh(s: &Spec, script: &str) -> Output {
    declass_sandbox::run(
        KIND,
        s,
        &["/bin/sh".into(), "-c".into(), script.into()],
        &s.workspace,
    )
    .await
    .unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// Answers one HTTP request with `proxied:<request line>`, recording the line.
async fn answer<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    mut s: S,
    seen: Arc<Mutex<Vec<String>>>,
) {
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match s.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    let line = String::from_utf8_lossy(&head)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    seen.lock().unwrap().push(line.clone());
    let body = format!("proxied:{line}\n");
    let _ = s
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await;
    let _ = s.shutdown().await;
}

/// A stand-in proxy and the route to it for this platform.
async fn stand_in() -> (Network, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let route = if KIND == SandboxKind::Seatbelt {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((s, _)) = listener.accept().await {
                tokio::spawn(answer(s, log.clone()));
            }
        });
        ProxyRoute::Loopback { port }
    } else {
        let (route, incoming) = bridge::channel(helper()).unwrap();
        tokio::spawn(async move {
            while let Some(Ok(s)) = incoming.next().await {
                tokio::spawn(answer(s, log.clone()));
            }
        });
        route
    };
    (Network::Proxy(route), seen)
}

#[tokio::test]
async fn with_the_proxy_route_a_command_reaches_the_proxy_and_nothing_else() {
    let (_d, ws) = workspace();
    let (network, seen) = stand_in().await;
    let s = spec(&ws, network);
    let o = sh(
        &s,
        "curl -sS -m 10 http://registry.example/pkg; \
         echo \"proxy=$HTTPS_PROXY no=$NO_PROXY\"; \
         curl -sS -m 5 --noproxy '*' -o /dev/null http://1.1.1.1/ && echo DIRECT-IP; \
         curl -sS -m 5 --noproxy '*' -o /dev/null https://example.com/ && echo DIRECT-NAME; \
         exit 3",
    )
    .await;
    let out = text(&o);
    assert!(
        out.contains("proxied:GET http://registry.example/pkg HTTP/1.1"),
        "{out}"
    );
    assert!(
        out.contains("proxy=http://127.0.0.1:") && out.contains("no=localhost,127.0.0.1,::1"),
        "{out}"
    );
    assert!(!out.contains("DIRECT-"), "{out}");
    // The command's own exit code comes through the bridge.
    assert_eq!(o.exit_code, Some(3), "{out}");
    assert_eq!(seen.lock().unwrap().len(), 1);
}

/// A development-server port nothing on the host listens on (Seatbelt allows
/// only those; under bubblewrap any port would do).
fn free_dev_port(besides: u16) -> u16 {
    declass_sandbox::DEV_PORTS
        .iter()
        .flat_map(|&(a, b)| a..=b)
        .filter(|p| *p != besides)
        .find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
        .expect("a free development port")
}

#[tokio::test]
async fn with_the_proxy_route_a_command_reaches_the_servers_it_starts() {
    let (_d, ws) = workspace();
    std::fs::write(ws.join("index.html"), "served-locally\n").unwrap();
    let (network, _seen) = stand_in().await;
    // Services of the host's own, listening before the command starts: on an
    // ephemeral port and on a development port.
    let host = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host_port = host.local_addr().unwrap().port();
    let mut dev_host = None;
    for p in declass_sandbox::DEV_PORTS.iter().flat_map(|&(a, b)| a..=b) {
        if let Ok(l) = tokio::net::TcpListener::bind(("127.0.0.1", p)).await {
            dev_host = Some(l);
            break;
        }
    }
    let dev_host = dev_host.expect("a free development port");
    let dev_host_port = dev_host.local_addr().unwrap().port();
    let port = free_dev_port(dev_host_port);
    let s = spec(&ws, network);
    let o = sh(
        &s,
        &format!(
            "python3 -m http.server {port} --bind 127.0.0.1 >/dev/null 2>&1 & \
             for i in 1 2 3 4 5 6 7 8 9 10; do \
               curl -sS -m 2 http://127.0.0.1:{port}/index.html 2>/dev/null && break; sleep 0.5; \
             done; \
             curl -sS -m 3 -o /dev/null http://localhost:{port}/ && echo BY-NAME; \
             curl -sS -m 3 -o /dev/null http://127.0.0.1:{host_port}/ && echo HOST-SERVICE; \
             curl -sS -m 3 -o /dev/null http://127.0.0.1:{dev_host_port}/ && echo HOST-DEV-SERVICE; \
             kill %1"
        ),
    )
    .await;
    let out = text(&o);
    assert!(out.contains("served-locally"), "{out}");
    assert!(out.contains("BY-NAME"), "{out}");
    assert!(!out.contains("HOST-"), "{out}");
    for (name, l) in [("ephemeral", &host), ("development", &dev_host)] {
        let accepted = tokio::time::timeout(Duration::from_millis(300), l.accept()).await;
        assert!(
            accepted.is_err(),
            "the command reached a host service on a {name} port"
        );
    }
}

#[tokio::test]
async fn without_network_a_command_reaches_not_even_the_proxy() {
    let (_d, ws) = workspace();
    let (Network::Proxy(route), seen) = stand_in().await else {
        unreachable!()
    };
    let port = match &route {
        ProxyRoute::Loopback { port } => *port,
        ProxyRoute::Bridge { .. } => 1,
    };
    let o = sh(
        &spec(&ws, Network::Off),
        &format!(
            "curl -sS -m 5 -x http://127.0.0.1:{port} http://registry.example/ && echo REACHED"
        ),
    )
    .await;
    assert!(!text(&o).contains("REACHED"), "{}", text(&o));
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_long_lived_server_cannot_start_with_a_proxy_route() {
    let (_d, ws) = workspace();
    let (network, seen) = stand_in().await;
    let s = spec(&ws, network);
    let argv = [
        "/bin/sh".into(),
        "-c".into(),
        "echo started > server-started; curl -sS -m 3 http://registry.example/secret".into(),
    ];
    let err = match declass_sandbox::spawn(KIND, &s, &argv, &ws).await {
        Err(err) => err,
        Ok(mut process) => {
            process.stop(Duration::from_secs(1)).await;
            panic!("a server started with the egress proxy route");
        }
    };
    assert!(
        err.to_string()
            .contains("server process cannot use the egress proxy")
    );
    assert!(!ws.join("server-started").exists());
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unix_sockets_stay_unreachable_with_the_proxy_route() {
    let (_d, ws) = workspace();
    let path = ws.join("engine.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let (network, _seen) = stand_in().await;
    let o = sh(
        &spec(&ws, network),
        &format!(
            "curl -s -m 3 --unix-socket {} http://engine/version && echo REACHED",
            path.display()
        ),
    )
    .await;
    assert!(!text(&o).contains("REACHED"), "{}", text(&o));
    let accepted = tokio::time::timeout(Duration::from_millis(300), listener.accept()).await;
    assert!(accepted.is_err(), "the command connected to the socket");
}

/// The bridge protocol itself, without a sandbox: the helper hands each
/// connection to the host, and passes the command's exit code on.
#[tokio::test]
async fn the_bridge_helper_hands_connections_to_the_host() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (ProxyRoute::Bridge { channel, helper }, incoming) = bridge::channel(helper()).unwrap()
    else {
        unreachable!()
    };
    let log = seen.clone();
    tokio::spawn(async move {
        while let Some(Ok(s)) = incoming.next().await {
            tokio::spawn(answer(s, log.clone()));
        }
    });
    let stdin = rustix::io::fcntl_dupfd_cloexec(&*channel, 0).unwrap();
    drop(channel);
    let out = tokio::process::Command::new(&helper[0])
        .args([
            "/bin/sh",
            "-c",
            "curl -sS -m 10 http://a.example/1; curl -sS -m 10 http://b.example/2; exit 5",
        ])
        .stdin(std::process::Stdio::from(stdin))
        .output()
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("proxied:GET http://a.example/1"), "{text}");
    assert!(text.contains("proxied:GET http://b.example/2"), "{text}");
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(seen.lock().unwrap().len(), 2);
}
