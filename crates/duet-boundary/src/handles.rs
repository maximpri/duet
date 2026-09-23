// SPDX-License-Identifier: GPL-3.0-or-later
//! Raw sensitive content, kept on this machine and referred to by handle.

use duet_fs::FsError;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct HandleInfo {
    pub id: String,
    pub source: String,
    pub bytes: usize,
    /// Whether the frontier may read raw ranges (public but bulky content).
    pub public: bool,
}

pub struct HandleStore {
    dir: PathBuf,
    next: u32,
    info: BTreeMap<String, HandleInfo>,
}

impl HandleStore {
    pub fn open(dir: &Path) -> Result<Self, FsError> {
        duet_fs::private::ensure_private_dir(dir)?;
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
                info.insert(
                    name.clone(),
                    HandleInfo {
                        id: name,
                        source,
                        bytes,
                        public: false,
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

    pub fn put(&mut self, bytes: &[u8], source: &str, public: bool) -> Result<HandleInfo, FsError> {
        let id = format!("h{}", self.next);
        self.next += 1;
        duet_fs::private::write_private(&self.dir.join(&id), bytes)?;
        duet_fs::private::write_private(&self.dir.join(format!("{id}.source")), source.as_bytes())?;
        let info = HandleInfo {
            id: id.clone(),
            source: source.to_owned(),
            bytes: bytes.len(),
            public,
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
        let h = s.put(b"raw log", "logs/prod.log", false).unwrap();
        assert_eq!(h.id, "h1");
        let s2 = HandleStore::open(&d.path().join("handles")).unwrap();
        assert_eq!(s2.get("h1").unwrap().1, b"raw log");
        let mut s2 = s2;
        assert_eq!(s2.put(b"x", "y", true).unwrap().id, "h2");
    }
}
