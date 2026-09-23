//! Webhook envelope parsing and routing.
//!
//! Envelope format: header lines `Name: value` (whitespace after the colon is optional; names
//! are case-insensitive), then a blank line, then the body.
//! A header line that starts with a space or tab continues the previous header's value
//! (joined with a single space). Required headers: `Event`, `Signature`, `Body-Length`.
//! `Signature` is `t=<unix seconds>,v1=<hex digest>`; extra comma-separated parts are
//! ignored. `Body-Length` is the body's length in bytes as a plain decimal integer.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub event: String,
    pub timestamp: u64,
    pub digest: String,
    /// Header values keyed by lower-case name.
    pub headers: HashMap<String, String>,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Billing,
    Crm,
    Treasury,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeError {
    MissingHeader(&'static str),
    BadSignature,
    BadLength,
    LengthMismatch { declared: usize, actual: usize },
    UnknownEvent(String),
}

fn parse_headers(head: &str) -> HashMap<String, String> {
    let mut headers: HashMap<String, String> = HashMap::new();
    let mut last: Option<String> = None;
    for line in head.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(value) = last.as_ref().and_then(|name| headers.get_mut(name)) {
                value.push(' ');
                value.push_str(line.trim());
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            headers.insert(name.clone(), value.trim().to_string());
            last = Some(name);
        }
    }
    headers
}

fn parse_signature(sig: &str) -> Result<(u64, String), EnvelopeError> {
    let mut timestamp = None;
    let mut digest = None;
    for part in sig.split(',') {
        match part.trim().split_once('=') {
            Some(("t", v)) => timestamp = v.parse::<u64>().ok(),
            Some(("v1", v)) if !v.is_empty() => digest = Some(v.to_string()),
            _ => {}
        }
    }
    timestamp.zip(digest).ok_or(EnvelopeError::BadSignature)
}

pub fn parse_envelope(raw: &str) -> Result<Envelope, EnvelopeError> {
    let (head, body) = raw.split_once("\n\n").unwrap_or((raw, ""));
    let headers = parse_headers(head);
    let get = |name: &'static str| headers.get(&name.to_ascii_lowercase()).ok_or(EnvelopeError::MissingHeader(name));
    let event = get("Event")?.clone();
    let (timestamp, digest) = parse_signature(get("Signature")?)?;
    let length = get("Body-Length")?;
    if length.is_empty() || !length.bytes().all(|b| b.is_ascii_digit()) {
        return Err(EnvelopeError::BadLength);
    }
    let declared: usize = length.parse().map_err(|_| EnvelopeError::BadLength)?;
    if declared != body.len() {
        return Err(EnvelopeError::LengthMismatch { declared, actual: body.len() });
    }
    Ok(Envelope { event, timestamp, digest, headers, body: body.to_string() })
}

pub fn route(envelope: &Envelope) -> Result<Route, EnvelopeError> {
    match envelope.event.split('.').next() {
        Some("invoice") => Ok(Route::Billing),
        Some("customer") => Ok(Route::Crm),
        Some("payout") => Ok(Route::Treasury),
        _ => Err(EnvelopeError::UnknownEvent(envelope.event.clone())),
    }
}
