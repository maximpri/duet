// SPDX-License-Identifier: GPL-3.0-or-later
//! Detector throughput on large mixed content (logs, code, JSON, lockfile
//! lines, prose, a few secrets): the imported rules must stay cheap enough to
//! scan every tool result and every outbound request.
//!
//! The quick test scans 1 MB and only reports. The full measurement is ignored
//! (debug builds say nothing about speed); run it optimized:
//!
//! ```sh
//! cargo test --release -p duet-boundary --test throughput -- --ignored --nocapture
//! ```

use duet_boundary::detect::{Detectors, scan_each};
use std::time::Instant;

/// A small deterministic generator (xorshift64*), so every run scans the same text.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }

    fn hex(&mut self, len: usize) -> String {
        (0..len)
            .map(|_| char::from(b"0123456789abcdef"[self.below(16)]))
            .collect()
    }

    fn alnum(&mut self, len: usize) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        (0..len)
            .map(|_| char::from(A[self.below(A.len())]))
            .collect()
    }
}

const PATHS: &[&str] = &[
    "/api/v1/orders",
    "/api/v1/invoices/export",
    "/healthz",
    "/api/v2/customers/search",
    "/static/app.js",
];
const CODE: &[&str] = &[
    "fn parse_header(line: &str) -> Result<Header, ParseError> {",
    "    let token = lexer.next_token()?;",
    "    let cfg = Config::from_sources(&env_file, &config_file)?;",
    "    if retries > MAX_RETRIES { return Err(Error::Timeout(elapsed)); }",
    "    for (index, record) in records.iter().enumerate() {",
    "        total_minor_units += record.amount_minor_units;",
    "impl<'a> Iterator for Chunks<'a> {",
    "    const API_VERSION: &str = \"2024-06-01\";",
    "    let access_token = request.headers().get(AUTHORIZATION);",
    "}",
    "    // Retry with exponential backoff; the key rotates hourly.",
    "export function useSession(options = {}) { return useContext(SessionContext); }",
    "def load_settings(path: str) -> dict[str, Any]:",
];
const PROSE: &[&str] = &[
    "The scheduler retries failed jobs with exponential backoff and records each attempt.",
    "See the migration guide for the new configuration keys and the removed options.",
    "Exports run nightly; the report lists invoices by region, currency and status.",
    "Use the access token from the environment, never a literal in source code.",
];

fn line(rng: &mut Rng, n: usize) -> String {
    match rng.below(10) {
        0..=3 => format!(
            "2026-09-25T10:{:02}:{:02}.{:03}Z {} request_id={} method=GET path={} status={} duration_ms={}",
            rng.below(60),
            rng.below(60),
            rng.below(1000),
            rng.pick(&["INFO", "WARN", "DEBUG", "ERROR"]),
            rng.hex(16),
            rng.pick(PATHS),
            rng.pick(&["200", "201", "404", "500"]),
            rng.below(900)
        ),
        4..=6 => rng.pick(CODE).to_owned(),
        7 => format!(
            "{{\"id\": {}, \"sku\": \"SKU-{}\", \"qty\": {}, \"updated\": \"2026-09-{:02}\"}}",
            rng.below(1_000_000),
            rng.below(99_999),
            rng.below(40),
            rng.below(28) + 1
        ),
        8 => format!(
            "checksum = \"{}\"  # {} v{}.{}.{}",
            rng.hex(64),
            rng.pick(&["serde", "tokio", "regex", "hyper"]),
            rng.below(3),
            rng.below(40),
            rng.below(20)
        ),
        _ if n.is_multiple_of(97) => format!("PAYMENTS_API_KEY=sk_live_{}", rng.alnum(28)),
        _ => rng.pick(PROSE).to_owned(),
    }
}

/// About `bytes` of mixed log and code lines.
fn corpus(bytes: usize) -> String {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut out = String::with_capacity(bytes + 256);
    let mut n = 0;
    while out.len() < bytes {
        out.push_str(&line(&mut rng, n));
        out.push('\n');
        n += 1;
    }
    out
}

/// Megabytes per second scanning `text` whole and in `piece`-byte pieces
/// (tool results are scanned one at a time), and the findings.
fn measure(text: &str, piece: usize) -> (f64, f64, usize) {
    let d = Detectors::default();
    // Compile every detector before timing.
    let _ = scan_each("warm up", d);
    let mb = text.len() as f64 / 1e6;
    let started = Instant::now();
    let found = scan_each(text, d).len();
    let whole = mb / started.elapsed().as_secs_f64();
    let started = Instant::now();
    let mut rest = text;
    while !rest.is_empty() {
        let mut cut = piece.min(rest.len());
        while !rest.is_char_boundary(cut) {
            cut += 1;
        }
        let cut = rest[..cut].rfind('\n').map_or(cut, |i| i + 1);
        let _ = scan_each(&rest[..cut], d);
        rest = &rest[cut..];
    }
    let pieces = mb / started.elapsed().as_secs_f64();
    (whole, pieces, found)
}

#[test]
fn quick_throughput_report() {
    let text = corpus(1_000_000);
    let (whole, pieces, found) = measure(&text, 8 * 1024);
    println!(
        "detector throughput (1 MB, this build): {whole:.1} MB/s whole, {pieces:.1} MB/s in 8 KB pieces, {found} findings"
    );
    assert!(found > 0, "the corpus plants secrets");
}

#[test]
#[ignore = "measurement; run with --release -- --ignored --nocapture"]
fn throughput_10mb() {
    let text = corpus(10_000_000);
    let (whole, pieces, found) = measure(&text, 8 * 1024);
    println!(
        "detector throughput (10 MB mixed log/code): {whole:.1} MB/s whole, {pieces:.1} MB/s in 8 KB pieces, {found} findings"
    );
}
