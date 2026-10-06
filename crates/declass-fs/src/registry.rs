// SPDX-License-Identifier: GPL-3.0-or-later
//! The single registry of paths Declass keeps under a workspace's `.declass/`.
//! Every `.declass/` path used anywhere in the code must be listed here; a test
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

pub const DECLASS_DIR: &str = ".declass";

pub const ENTRIES: &[Entry] = &[
    Entry {
        path: ".declass/derived.json",
        lifetime: Lifetime::Preserved,
        sensitive: true,
        purpose: "workspace-wide paths derived from sensitive data",
    },
    Entry {
        path: ".declass/derived.pending",
        lifetime: Lifetime::Preserved,
        sensitive: true,
        purpose: "incomplete sensitive-command classification marker",
    },
    Entry {
        path: ".declass/skills",
        lifetime: Lifetime::Preserved,
        sensitive: true,
        purpose: "project skill instructions and resources",
    },
    Entry {
        path: ".declass/config.toml",
        lifetime: Lifetime::Preserved,
        sensitive: false,
        purpose: "project settings (tighten-only)",
    },
    Entry {
        path: ".declass/git",
        lifetime: Lifetime::Preserved,
        sensitive: true,
        purpose: "private checkpoint store",
    },
    Entry {
        path: ".declass/runs",
        lifetime: Lifetime::Retained,
        sensitive: true,
        purpose: "per-run transcript, handles, vault, spill, write journal",
    },
    Entry {
        path: ".declass/audit",
        lifetime: Lifetime::Retained,
        sensitive: false,
        purpose: "hash-chained outbound audit logs (placeholder-substituted)",
    },
    Entry {
        path: ".declass/lock",
        lifetime: Lifetime::Transient,
        sensitive: false,
        purpose: "workspace lock",
    },
    Entry {
        path: ".declass/tmp",
        lifetime: Lifetime::Transient,
        sensitive: true,
        purpose: "host scratch space",
    },
];

/// The `.declass/` directory itself.
pub const ROOT_ENTRY: Entry = Entry {
    path: ".declass/",
    lifetime: Lifetime::Preserved,
    sensitive: true,
    purpose: "Declass state directory",
};

pub fn lookup(path: &str) -> Option<&'static Entry> {
    if path == ".declass/" || path == ".declass" {
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
            lookup(".declass/runs/r1/vault.json").unwrap().path,
            ".declass/runs"
        );
        assert!(lookup(".declass/runsx").is_none());
        assert!(lookup(".declass/unknown").is_none());
    }

    /// Fails when any crate uses a `.declass/` path that is not registered.
    #[test]
    fn every_declass_path_in_the_sources_is_registered() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut unregistered = Vec::new();
        let mut stack = vec![crates.to_path_buf()];
        while let Some(dir) = stack.pop() {
            // Other tests create and delete temporary directories while this walks.
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
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
                for (i, _) in text.match_indices("\".declass/") {
                    let lit: String = text[i + 1..].chars().take_while(|c| *c != '"').collect();
                    if lookup(&lit).is_none() {
                        unregistered.push(format!("{}: {lit}", p.display()));
                    }
                }
            }
        }
        assert!(
            unregistered.is_empty(),
            "unregistered .declass paths: {unregistered:#?}"
        );
    }
}
