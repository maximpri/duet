// SPDX-License-Identifier: GPL-3.0-or-later
//! A scripted HTTP/1.1 server on loopback, for tests of probing, discovery and
//! `duet doctor`. Each route answers a fixed status and JSON body; every
//! request is recorded (method, path, whether it carried a bearer token).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub bearer: bool,
}

pub struct MockServer {
    pub port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
}

impl MockServer {
    /// Serves `routes` (`"GET /v1/models"` → (status, body)); anything else is 404.
    pub fn start(routes: &[(&str, u16, &str)]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let routes: Arc<HashMap<String, (u16, String)>> = Arc::new(
            routes
                .iter()
                .map(|(r, s, b)| ((*r).to_owned(), (*s, (*b).to_owned())))
                .collect(),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen2, stop2) = (seen.clone(), stop.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let (routes, seen) = (routes.clone(), seen2.clone());
                std::thread::spawn(move || serve(stream, &routes, &seen));
            }
        });
        Self { port, seen, stop }
    }

    /// `http://127.0.0.1:<port>/v1`.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn serve(stream: TcpStream, routes: &HashMap<String, (u16, String)>, seen: &Mutex<Vec<Seen>>) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let (mut length, mut bearer) = (0usize, false);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
        if lower.starts_with("authorization: bearer ") {
            bearer = true;
        }
    }
    let mut body = vec![0; length];
    let _ = reader.read_exact(&mut body);
    if let Ok(mut s) = seen.lock() {
        s.push(Seen {
            method: method.clone(),
            path: path.clone(),
            bearer,
        });
    }
    let (status, body) = routes
        .get(&format!("{method} {path}"))
        .cloned()
        .unwrap_or((404, r#"{"error":"not found"}"#.to_owned()));
    let mut out = stream;
    let _ = write!(
        out,
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
}
