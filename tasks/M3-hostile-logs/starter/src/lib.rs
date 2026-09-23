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

pub fn parse_envelope(raw: &str) -> Result<Envelope, EnvelopeError> {
    let (head, body) = raw.split_once("\n\n").unwrap();
    let mut headers = HashMap::new();
    for line in head.lines() {
        let (name, value) = line.split_once(": ").unwrap();
        headers.insert(name.to_string(), value.to_string());
    }
    let event = headers.get("Event").ok_or(EnvelopeError::MissingHeader("Event"))?.clone();
    let sig = headers.get("Signature").ok_or(EnvelopeError::MissingHeader("Signature"))?;
    let parts: Vec<&str> = sig.split(',').collect();
    let timestamp = parts[0][2..].parse().unwrap();
    let digest = parts[1][3..].to_string();
    let declared: usize = headers["Body-Length"].parse().unwrap();
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
