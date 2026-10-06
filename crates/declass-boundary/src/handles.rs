// SPDX-License-Identifier: GPL-3.0-or-later
//! Content kept on this machine and referred to by handle: raw sensitive
//! content, and public content too bulky to show whole.

use declass_fs::FsError;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct HandleInfo {
    pub id: String,
    pub source: String,
    pub bytes: usize,
    /// Whether the frontier may read raw ranges (public but bulky content).
    pub public: bool,
    /// Line number of the content's first line in its source (a file read
    /// from line 120 holds lines 120..), so ranges use the source's numbers.
    pub first_line: usize,
}

pub struct HandleStore {
    dir: PathBuf,
    next: u32,
    info: BTreeMap<String, HandleInfo>,
}

impl HandleStore {
    pub fn open(dir: &Path) -> Result<Self, FsError> {
        declass_fs::private::ensure_private_dir(dir)?;
        let mut next = 1;
        let mut info = BTreeMap::new();
        for entry in std::fs::read_dir(dir)
            .map_err(|e| FsError::io("list", dir, e))?
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(n) = name.strip_prefix('h').and_then(|n| n.parse::<u32>().ok()) {
                next = next.max(n + 1);
                let bytes = entry.metadata().map_or(0, |m| m.len() as usize);
                let source =
                    std::fs::read_to_string(dir.join(format!("{name}.source"))).unwrap_or_default();
                // Only a handle stored as public has this marker; anything else stays sensitive.
                let public = std::fs::read_to_string(dir.join(format!("{name}.public")))
                    .ok()
                    .and_then(|s| s.trim().parse::<usize>().ok());
                info.insert(
                    name.clone(),
                    HandleInfo {
                        id: name,
                        source,
                        bytes,
                        public: public.is_some(),
                        first_line: public.unwrap_or(1).max(1),
                    },
                );
            }
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            next,
            info,
        })
    }

    /// Stores sensitive content (never readable raw by the frontier).
    pub fn put(&mut self, bytes: &[u8], source: &str) -> Result<HandleInfo, FsError> {
        self.store(bytes, source, None)
    }

    /// Stores public content whose first line is line `first_line` of its source.
    pub fn put_public(
        &mut self,
        bytes: &[u8],
        source: &str,
        first_line: usize,
    ) -> Result<HandleInfo, FsError> {
        self.store(bytes, source, Some(first_line.max(1)))
    }

    fn store(
        &mut self,
        bytes: &[u8],
        source: &str,
        public: Option<usize>,
    ) -> Result<HandleInfo, FsError> {
        let id = format!("h{}", self.next);
        self.next += 1;
        declass_fs::private::write_private(&self.dir.join(&id), bytes)?;
        declass_fs::private::write_private(
            &self.dir.join(format!("{id}.source")),
            source.as_bytes(),
        )?;
        if let Some(first) = public {
            declass_fs::private::write_private(
                &self.dir.join(format!("{id}.public")),
                first.to_string().as_bytes(),
            )?;
        }
        let info = HandleInfo {
            id: id.clone(),
            source: source.to_owned(),
            bytes: bytes.len(),
            public: public.is_some(),
            first_line: public.unwrap_or(1),
        };
        self.info.insert(id, info.clone());
        Ok(info)
    }

    pub fn get(&self, id: &str) -> Option<(HandleInfo, Vec<u8>)> {
        let info = self.info.get(id)?.clone();
        std::fs::read(self.dir.join(id)).ok().map(|b| (info, b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_reopens() {
        let d = tempfile::tempdir().unwrap();
        let mut s = HandleStore::open(&d.path().join("handles")).unwrap();
        let h = s.put(b"raw log", "logs/prod.log").unwrap();
        assert_eq!(h.id, "h1");
        let s2 = HandleStore::open(&d.path().join("handles")).unwrap();
        assert_eq!(s2.get("h1").unwrap().1, b"raw log");
        let mut s2 = s2;
        assert_eq!(s2.put_public(b"x", "y", 1).unwrap().id, "h2");
    }

    #[test]
    fn public_flag_and_first_line_survive_a_reopen() {
        let d = tempfile::tempdir().unwrap();
        let mut s = HandleStore::open(&d.path().join("handles")).unwrap();
        s.put(b"secret", "logs/a.log").unwrap();
        s.put_public(b"fn a() {}\n", "src/a.rs", 120).unwrap();
        let s = HandleStore::open(&d.path().join("handles")).unwrap();
        let (sensitive, _) = s.get("h1").unwrap();
        let (public, _) = s.get("h2").unwrap();
        assert!(!sensitive.public);
        assert!(public.public);
        assert_eq!(public.first_line, 120);
    }
}
