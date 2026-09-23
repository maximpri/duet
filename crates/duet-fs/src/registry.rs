// SPDX-License-Identifier: GPL-3.0-or-later
//! The single registry of paths Duet keeps under a workspace's `.duet/`.
//! Every `.duet/` path used anywhere in the code must be listed here; a test
//! scans the sources and fails on unlisted ones.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    /// Kept until the owner deletes it.
    Preserved,
    /// Deleted after the retention period (`retention_days` / `audit_retention_days`).
    Retained,
    /// Removed at the end of a run or on the next start.
    Transient,
}

pub struct Entry {
    pub path: &'static str,
    pub lifetime: Lifetime,
    /// Holds raw sensitive content (handles, vault, transcripts): mode 0700/0600.
    pub sensitive: bool,
    pub purpose: &'static str,
}

pub const DUET_DIR: &str = ".duet";

pub const ENTRIES: &[Entry] = &[
    Entry {
        path: ".duet/config.toml",
        lifetime: Lifetime::Preserved,
        sensitive: false,
        purpose: "project settings (tighten-only)",
    },
    Entry {
        path: ".duet/git",
        lifetime: Lifetime::Preserved,
        sensitive: true,
        purpose: "private checkpoint store",
    },
    Entry {
        path: ".duet/runs",
        lifetime: Lifetime::Retained,
        sensitive: true,
        purpose: "per-run transcript, handles, vault, spill, write journal",
    },
    Entry {
        path: ".duet/audit",
        lifetime: Lifetime::Retained,
        sensitive: false,
        purpose: "hash-chained outbound audit logs (placeholder-substituted)",
    },
    Entry {
        path: ".duet/lock",
        lifetime: Lifetime::Transient,
        sensitive: false,
        purpose: "workspace lock",
    },
    Entry {
        path: ".duet/tmp",
        lifetime: Lifetime::Transient,
        sensitive: true,
        purpose: "host scratch space",
    },
];

/// The `.duet/` directory itself.
pub const ROOT_ENTRY: Entry = Entry {
    path: ".duet/",
    lifetime: Lifetime::Preserved,
    sensitive: true,
    purpose: "Duet state directory",
};

pub fn lookup(path: &str) -> Option<&'static Entry> {
    if path == ".duet/" || path == ".duet" {
        return Some(&ROOT_ENTRY);
    }
    ENTRIES.iter().find(|e| {
        path == e.path
            || path
                .strip_prefix(e.path)
                .is_some_and(|r| r.starts_with('/'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn lookup_covers_nested_paths() {
        assert_eq!(
            lookup(".duet/runs/r1/vault.json").unwrap().path,
            ".duet/runs"
        );
        assert!(lookup(".duet/runsx").is_none());
        assert!(lookup(".duet/unknown").is_none());
    }

    /// Fails when any crate uses a `.duet/` path that is not registered.
    #[test]
    fn every_duet_path_in_the_sources_is_registered() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut unregistered = Vec::new();
        let mut stack = vec![crates.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    if p.file_name().is_some_and(|n| n != "target") {
                        stack.push(p);
                    }
                    continue;
                }
                if p.extension().is_none_or(|e| e != "rs") || p.ends_with("registry.rs") {
                    continue;
                }
                let full = std::fs::read_to_string(&p).unwrap();
                // Production code only; test fixtures may use arbitrary paths.
                let text = full.split("#[cfg(").next().unwrap_or_default();
                for (i, _) in text.match_indices("\".duet/") {
                    let lit: String = text[i + 1..].chars().take_while(|c| *c != '"').collect();
                    if lookup(&lit).is_none() {
                        unregistered.push(format!("{}: {lit}", p.display()));
                    }
                }
            }
        }
        assert!(
            unregistered.is_empty(),
            "unregistered .duet paths: {unregistered:#?}"
        );
    }
}
