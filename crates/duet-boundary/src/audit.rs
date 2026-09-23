// SPDX-License-Identifier: GPL-3.0-or-later
//! Hash-chained log of every request sent to the frontier.
//!
//! Each record carries the SHA-256 of the previous record, so any edit,
//! deletion or reordering breaks verification. Requests are stored after the
//! gate's substitutions: the log never holds raw sensitive values.

use duet_fs::FsError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditRecord {
    pub seq: u64,
    pub prev: String,
    pub unix_ms: u128,
    pub endpoint: String,
    pub model: String,
    /// SHA-256 of `request`.
    pub request_sha256: String,
    /// The exact JSON body sent (after substitutions).
    pub request: serde_json::Value,
    /// What the gate replaced or blocked before sending.
    pub interventions: Vec<String>,
}

impl AuditRecord {
    pub fn digest(&self) -> String {
        hex::encode(Sha256::digest(serde_json::to_vec(self).unwrap_or_default()))
    }
}

pub struct AuditLog {
    path: PathBuf,
    seq: u64,
    prev: String,
}

impl AuditLog {
    /// Opens (or creates) the log, continuing an existing chain.
    pub fn open(path: &Path) -> Result<Self, FsError> {
        if let Some(parent) = path.parent() {
            duet_fs::private::ensure_private_dir(parent)?;
        }
        let lines = duet_fs::private::read_lines_repairing(path)?;
        let (seq, prev) = match lines
            .last()
            .and_then(|l| serde_json::from_str::<AuditRecord>(l).ok())
        {
            Some(last) => (last.seq, last.digest()),
            None => (0, GENESIS.to_owned()),
        };
        Ok(Self {
            path: path.to_path_buf(),
            seq,
            prev,
        })
    }

    pub fn append(
        &mut self,
        endpoint: &str,
        model: &str,
        request: serde_json::Value,
        interventions: Vec<String>,
    ) -> Result<AuditRecord, FsError> {
        let body = serde_json::to_vec(&request).unwrap_or_default();
        let record = AuditRecord {
            seq: self.seq + 1,
            prev: self.prev.clone(),
            unix_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis()),
            endpoint: endpoint.to_owned(),
            model: model.to_owned(),
            request_sha256: hex::encode(Sha256::digest(&body)),
            request,
            interventions,
        };
        duet_fs::private::append_line(
            &self.path,
            &serde_json::to_string(&record).unwrap_or_default(),
        )?;
        self.seq = record.seq;
        self.prev = record.digest();
        Ok(record)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verification {
    Intact { records: u64 },
    Broken { at_seq: u64, reason: String },
}

/// Recomputes the chain.
pub fn verify(path: &Path) -> Result<Verification, FsError> {
    let text = std::fs::read_to_string(path).map_err(|e| FsError::io("read", path, e))?;
    let mut prev = GENESIS.to_owned();
    let mut expected_seq = 1;
    for line in text.lines().filter(|l| !l.is_empty()) {
        let Ok(r) = serde_json::from_str::<AuditRecord>(line) else {
            return Ok(Verification::Broken {
                at_seq: expected_seq,
                reason: "unparseable record".into(),
            });
        };
        let body = serde_json::to_vec(&r.request).unwrap_or_default();
        let reason = if r.seq != expected_seq {
            Some(format!("expected seq {expected_seq}, found {}", r.seq))
        } else if r.prev != prev {
            Some("previous-record hash does not match".into())
        } else if hex::encode(Sha256::digest(&body)) != r.request_sha256 {
            Some("request body does not match its hash".into())
        } else {
            None
        };
        if let Some(reason) = reason {
            return Ok(Verification::Broken {
                at_seq: expected_seq,
                reason,
            });
        }
        prev = r.digest();
        expected_seq += 1;
    }
    Ok(Verification::Intact {
        records: expected_seq - 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chain_verifies_and_detects_tampering() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("audit/r1.jsonl");
        let mut log = AuditLog::open(&p).unwrap();
        for i in 0..3 {
            log.append("https://f/v1", "m", json!({"n": i}), vec![])
                .unwrap();
        }
        // Reopening continues the same chain.
        let mut again = AuditLog::open(&p).unwrap();
        again
            .append("https://f/v1", "m", json!({"n": 3}), vec!["x".into()])
            .unwrap();
        assert_eq!(verify(&p).unwrap(), Verification::Intact { records: 4 });

        let text = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&p, text.replacen("\"n\":1", "\"n\":9", 1)).unwrap();
        assert!(matches!(
            verify(&p).unwrap(),
            Verification::Broken { at_seq: 2, .. }
        ));

        let lines: Vec<&str> = text.lines().collect();
        std::fs::write(&p, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        assert!(matches!(
            verify(&p).unwrap(),
            Verification::Broken { at_seq: 2, .. }
        ));
    }
}
