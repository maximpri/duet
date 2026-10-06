// SPDX-License-Identifier: GPL-3.0-or-later
//! Parent-directory handles opened without following links.
//!
//! Every component is opened with `openat(O_NOFOLLOW)` from the workspace root,
//! so a symlink planted anywhere on the path cannot redirect a read or write.

use crate::error::FsError;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub struct PinnedParent {
    dir: File,
    name: OsString,
    path: PathBuf,
}

const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

impl PinnedParent {
    /// Opens the parent of `rel` under `root`. With `create`, missing parent
    /// directories are created (mode 0755).
    pub fn open(root: &Path, rel: &Path, create: bool) -> Result<Self, FsError> {
        let name = rel
            .file_name()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| FsError::Outside(rel.display().to_string()))?
            .to_owned();
        let root_fd = rustix::fs::open(root, DIR_FLAGS, Mode::empty())
            .map_err(|e| FsError::io("open workspace root", root, e))?;
        let mut dir: File = root_fd.into();
        let mut shown = root.to_path_buf();
        if let Some(parent) = rel.parent() {
            for c in parent.components() {
                let Component::Normal(part) = c else {
                    return Err(FsError::Outside(rel.display().to_string()));
                };
                shown.push(part);
                dir = open_child(&dir, part, &shown, create)?;
            }
        }
        Ok(Self {
            dir,
            name,
            path: root.join(rel),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn file_type(&self) -> Result<Option<FileType>, FsError> {
        match rustix::fs::statat(&self.dir, &self.name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) => Ok(Some(FileType::from_raw_mode(st.st_mode))),
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(e) => Err(FsError::io("inspect", &self.path, e)),
        }
    }

    /// Opens the leaf for reading; refuses links, FIFOs and directories.
    pub fn open_read(&self) -> Result<File, FsError> {
        match self.file_type()? {
            Some(FileType::RegularFile) => {}
            Some(FileType::Symlink) => return Err(FsError::Symlink(self.path.clone())),
            Some(_) => return Err(FsError::NotAFile(self.path.display().to_string())),
            None => return Err(FsError::io("open", &self.path, "no such file")),
        }
        let fd = rustix::fs::openat(
            &self.dir,
            &self.name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| FsError::io("open", &self.path, e))?;
        Ok(fd.into())
    }

    pub fn create_temp(&self, name: &OsStr, mode: u32) -> Result<File, FsError> {
        let fd = rustix::fs::openat(
            &self.dir,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(mode as _),
        )
        .map_err(|e| FsError::io("create temporary file", &self.path, e))?;
        Ok(fd.into())
    }

    pub fn rename_into_place(&self, temp: &OsStr) -> Result<(), FsError> {
        rustix::fs::renameat(&self.dir, temp, &self.dir, &self.name)
            .map_err(|e| FsError::io("replace", &self.path, e))
    }

    pub fn remove(&self) -> Result<(), FsError> {
        rustix::fs::unlinkat(&self.dir, &self.name, AtFlags::empty())
            .map_err(|e| FsError::io("remove", &self.path, e))
    }

    pub fn remove_temp(&self, temp: &OsStr) {
        let _ = rustix::fs::unlinkat(&self.dir, temp, AtFlags::empty());
    }

    pub fn sync(&self) -> Result<(), FsError> {
        rustix::fs::fsync(&self.dir).map_err(|e| FsError::io("sync directory", &self.path, e))
    }
}

fn open_child(parent: &File, name: &OsStr, shown: &Path, create: bool) -> Result<File, FsError> {
    match rustix::fs::openat(parent, name, DIR_FLAGS, Mode::empty()) {
        Ok(fd) => Ok(fd.into()),
        Err(rustix::io::Errno::NOENT) if create => {
            match rustix::fs::mkdirat(parent, name, Mode::from_raw_mode(0o755)) {
                Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                Err(e) => return Err(FsError::io("create directory", shown, e)),
            }
            rustix::fs::openat(parent, name, DIR_FLAGS, Mode::empty())
                .map(Into::into)
                .map_err(|e| classify(parent, name, shown, e))
        }
        Err(e) => Err(classify(parent, name, shown, e)),
    }
}

fn classify(parent: &File, name: &OsStr, shown: &Path, e: rustix::io::Errno) -> FsError {
    let is_link = rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
        .is_ok_and(|st| FileType::from_raw_mode(st.st_mode) == FileType::Symlink);
    if is_link {
        FsError::Symlink(shown.to_path_buf())
    } else if e == rustix::io::Errno::NOTDIR {
        FsError::NotAFile(shown.display().to_string())
    } else {
        FsError::io("open directory", shown, e)
    }
}
