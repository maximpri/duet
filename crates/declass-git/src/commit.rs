// SPDX-License-Identifier: GPL-3.0-or-later
//! Commits to the user's repository, and what they need: the current commit,
//! the operator's identity, and which paths may not be committed.
//!
//! A commit is built with plumbing only (`hash-object`, a private index,
//! `write-tree`, `commit-tree`, `update-ref`), so no hook, filter or editor
//! can run and nothing else changes: no staging of other files, no branch
//! switch, no checkout, no push. The content is committed byte for byte as it
//! is on disk.

use crate::{Git, GitError};
use std::path::{Path, PathBuf};

/// A commit made by [`Git::commit_paths`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    pub hash: String,
    /// The commit it follows (`None`: the first commit of the branch).
    pub parent: Option<String>,
    /// The branch it was recorded on (`None`: detached HEAD).
    pub branch: Option<String>,
}

/// Who a commit is by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

impl Identity {
    /// Parses `Name <email>`.
    pub fn parse(text: &str) -> Option<Self> {
        let (name, rest) = text.trim().split_once('<')?;
        let email = rest.strip_suffix('>')?.trim();
        let name = name.trim();
        let bad = |s: &str| s.is_empty() || s.contains(['<', '>', '\n', '\0']);
        if bad(name) || bad(email) {
            return None;
        }
        Some(Self {
            name: name.to_owned(),
            email: email.to_owned(),
        })
    }
}

/// The owner's git configuration files, in the order git reads them (a later
/// file wins): `$XDG_CONFIG_HOME/git/config` (or `~/.config/git/config`),
/// then `~/.gitconfig`.
pub fn user_config_files() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".config")));
    xdg.map(|x| x.join("git/config"))
        .into_iter()
        .chain(home.map(|h| h.join(".gitconfig")))
        .collect()
}

impl Git {
    fn text(&self, cwd: &Path, args: &[&str]) -> Result<String, GitError> {
        let out = self.run(cwd, args, &[], None)?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    /// The current commit, or `None` on an unborn branch.
    pub fn head(&self, workspace: &Path) -> Result<Option<String>, GitError> {
        let out = self.run_accepting(
            workspace,
            &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
            &[],
            None,
            &[0, 1],
        )?;
        let hash = String::from_utf8_lossy(&out).trim().to_owned();
        Ok(Some(hash).filter(|h| !h.is_empty()))
    }

    /// The workspace's directory inside the repository (`""` at its root,
    /// else ending in `/`).
    pub fn prefix(&self, workspace: &Path) -> Result<String, GitError> {
        self.text(workspace, &["rev-parse", "--show-prefix"])
    }

    /// The operator's identity from git configuration: the repository's own
    /// `user.name`/`user.email`, else the first of `user_files` (read later
    /// wins, as git does) that sets them. Only these two keys are read, with
    /// `--file`, so no include or other setting of those files takes effect.
    pub fn operator_identity(&self, workspace: &Path, user_files: &[PathBuf]) -> Option<Identity> {
        let get = |scope: &[&str], key: &str| {
            let mut args = vec!["config"];
            args.extend_from_slice(scope);
            args.extend(["--get", key]);
            self.text(workspace, &args).ok().filter(|v| !v.is_empty())
        };
        let pair = |scope: &[&str]| {
            Some(Identity {
                name: get(scope, "user.name")?,
                email: get(scope, "user.email")?,
            })
        };
        if let Some(id) = pair(&["--local"]) {
            return Some(id);
        }
        user_files
            .iter()
            .rev()
            .filter(|f| f.is_file())
            .find_map(|f| pair(&["--file", f.to_str()?]))
    }

    /// Why each of `paths` may not be committed, if it may not: git ignores
    /// it, a `filter` attribute (such as LFS) would transform it, or it is not
    /// a regular file or a deletion.
    pub fn commit_blockers(
        &self,
        workspace: &Path,
        paths: &[String],
    ) -> Result<Vec<(String, String)>, GitError> {
        let mut out = Vec::new();
        if paths.is_empty() {
            return Ok(out);
        }
        let mut list = Vec::new();
        for p in paths {
            list.extend_from_slice(p.as_bytes());
            list.push(0);
        }
        let ignored = self.run_accepting(
            workspace,
            &["check-ignore", "--no-index", "-z", "--stdin"],
            &[],
            Some(&list),
            &[0, 1],
        )?;
        for p in ignored.split(|&b| b == 0).filter(|p| !p.is_empty()) {
            out.push((
                String::from_utf8_lossy(p).into_owned(),
                "git ignores it".to_owned(),
            ));
        }
        let attrs = self.run(
            workspace,
            &["check-attr", "-z", "--stdin", "filter"],
            &[],
            Some(&list),
        )?;
        let fields: Vec<&[u8]> = attrs.split(|&b| b == 0).collect();
        for rec in fields.chunks(3) {
            if let [path, _, value] = rec
                && !matches!(*value, b"unspecified" | b"unset")
            {
                out.push((
                    String::from_utf8_lossy(path).into_owned(),
                    format!(
                        "it has the git filter `{}` (commit it yourself)",
                        String::from_utf8_lossy(value)
                    ),
                ));
            }
        }
        for p in paths {
            match std::fs::symlink_metadata(workspace.join(p)) {
                Ok(m) if m.is_file() => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => out.push((p.clone(), "it is not a regular file".to_owned())),
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// Commits the working-tree content of exactly `paths` (a missing file is
    /// committed as deleted) on top of the current commit, by `author` as
    /// author and committer. Other staged or unstaged changes are left as
    /// they are; the index entries of `paths` are updated to the commit.
    /// `index` is a scratch file for the private index (removed afterwards).
    /// Fails without changing anything when the commit would change nothing
    /// or HEAD moved while it was built.
    pub fn commit_paths(
        &self,
        workspace: &Path,
        index: &Path,
        paths: &[String],
        message: &str,
        author: &Identity,
    ) -> Result<Committed, GitError> {
        let refuse = |message: &str| GitError::Failed {
            args: "commit".into(),
            message: message.into(),
        };
        if paths.is_empty() {
            return Err(refuse("no paths to commit"));
        }
        let parent = self.head(workspace)?;
        let index_s = index.to_string_lossy().into_owned();
        let _ = std::fs::remove_file(index);
        let private = [("GIT_INDEX_FILE", index_s.as_str())];
        let result = (|| {
            if let Some(p) = &parent {
                self.run(workspace, &["read-tree", p], &private, None)?;
            }
            let mut entries = Vec::new();
            for p in paths {
                let abs = workspace.join(p);
                let entry = match std::fs::symlink_metadata(&abs) {
                    Ok(m) if m.is_file() => {
                        use std::os::unix::fs::PermissionsExt;
                        let mode = if m.permissions().mode() & 0o111 != 0 {
                            "100755"
                        } else {
                            "100644"
                        };
                        let blob =
                            self.text(workspace, &["hash-object", "-w", "--no-filters", "--", p])?;
                        Some(format!("{mode},{blob},{p}"))
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    _ => return Err(refuse(&format!("{p} is not a regular file"))),
                };
                entries.push((p, entry));
            }
            for (p, entry) in &entries {
                self.run(
                    workspace,
                    &update_index(p, entry.as_deref()),
                    &private,
                    None,
                )?;
            }
            let tree = self.text_env(workspace, &["write-tree"], &private)?;
            if let Some(p) = &parent {
                let old = self.text(workspace, &["rev-parse", &format!("{p}^{{tree}}")])?;
                if old == tree {
                    return Err(refuse(
                        "nothing to commit: these paths already match the current commit",
                    ));
                }
            }
            let mut args = vec!["commit-tree", tree.as_str()];
            if let Some(p) = &parent {
                args.extend(["-p", p.as_str()]);
            }
            let who = [
                ("GIT_AUTHOR_NAME", author.name.as_str()),
                ("GIT_AUTHOR_EMAIL", author.email.as_str()),
                ("GIT_COMMITTER_NAME", author.name.as_str()),
                ("GIT_COMMITTER_EMAIL", author.email.as_str()),
            ];
            let message = format!("{}\n", message.trim_end());
            let out = self.run(workspace, &args, &who, Some(message.as_bytes()))?;
            Ok((String::from_utf8_lossy(&out).trim().to_owned(), entries))
        })();
        let _ = std::fs::remove_file(index);
        let (hash, entries) = result?;
        // Compare-and-swap: a HEAD that moved meanwhile is left alone.
        let old = parent.clone().unwrap_or_default();
        self.run(
            workspace,
            &[
                "update-ref",
                "-m",
                "declass: git_commit",
                "HEAD",
                &hash,
                &old,
            ],
            &[],
            None,
        )?;
        // The real index follows the commit for these paths only.
        for (p, entry) in &entries {
            self.run(workspace, &update_index(p, entry.as_deref()), &[], None)?;
        }
        let branch = self
            .run_accepting(
                workspace,
                &["symbolic-ref", "--quiet", "--short", "HEAD"],
                &[],
                None,
                &[0, 1],
            )
            .map(|o| String::from_utf8_lossy(&o).trim().to_owned())
            .ok()
            .filter(|b| !b.is_empty());
        Ok(Committed {
            hash,
            parent,
            branch,
        })
    }

    fn text_env(
        &self,
        cwd: &Path,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<String, GitError> {
        let out = self.run(cwd, args, env, None)?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }
}

/// The `update-index` call that sets `path` to `entry` (`mode,blob,path`), or
/// removes it when `None`.
fn update_index<'a>(path: &'a str, entry: Option<&'a str>) -> Vec<&'a str> {
    match entry {
        Some(info) => vec!["update-index", "--add", "--cacheinfo", info],
        None => vec!["update-index", "--force-remove", "--", path],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CheckpointStore;

    fn git_in(ws: &Path, git: &Git, args: &[&str]) -> String {
        String::from_utf8_lossy(&git.run(ws, args, &[], None).unwrap())
            .trim()
            .to_owned()
    }

    fn repo() -> (tempfile::TempDir, PathBuf, Git) {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let git = Git::locate().unwrap();
        git_in(&ws, &git, &["init", "-q", "-b", "main"]);
        std::fs::write(ws.join("a.txt"), "one\n").unwrap();
        std::fs::write(ws.join("b.txt"), "bee\n").unwrap();
        git_in(&ws, &git, &["add", "-A"]);
        git_in(&ws, &git, &["commit", "-q", "-m", "init"]);
        (d, ws, git)
    }

    fn op() -> Identity {
        Identity::parse("Ada Operator <ada@example.test>").unwrap()
    }

    #[test]
    fn identities_parse_strictly() {
        assert_eq!(op().name, "Ada Operator");
        for bad in ["", "Ada", "<a@b>", "Ada <>", "Ada <a@b", "A<b> <c@d>"] {
            assert!(Identity::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn commits_exactly_the_given_paths_as_the_operator() {
        let (d, ws, git) = repo();
        let marker = ws.join("hooked");
        for hook in [
            "pre-commit",
            "commit-msg",
            "post-commit",
            "reference-transaction",
        ] {
            let path = ws.join(".git/hooks").join(hook);
            std::fs::write(&path, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
        }
        std::fs::write(ws.join("a.txt"), "two\n").unwrap();
        std::fs::write(ws.join("new.txt"), "fresh\n").unwrap();
        // The user's own staged and unstaged work stays as it is.
        std::fs::write(ws.join("b.txt"), "staged by the user\n").unwrap();
        git_in(&ws, &git, &["add", "b.txt"]);
        let before = git.head(&ws).unwrap().unwrap();
        let index = d.path().join("commit.index");
        let c = git
            .commit_paths(
                &ws,
                &index,
                &["a.txt".into(), "new.txt".into()],
                "Change a, add new\n\n",
                &op(),
            )
            .unwrap();
        assert_eq!(c.parent.as_deref(), Some(before.as_str()));
        assert_eq!(c.branch.as_deref(), Some("main"));
        assert_eq!(git.head(&ws).unwrap().as_deref(), Some(c.hash.as_str()));
        assert!(!marker.exists(), "a hook ran");
        assert!(!index.exists());
        let who = git_in(&ws, &git, &["log", "-1", "--format=%an <%ae>|%cn <%ce>|%B"]);
        assert_eq!(
            who,
            "Ada Operator <ada@example.test>|Ada Operator <ada@example.test>|Change a, add new"
        );
        let files = git_in(&ws, &git, &["show", "--format=", "--name-only", "HEAD"]);
        assert_eq!(files, "a.txt\nnew.txt");
        // Committed paths are clean; the user's staged change is still staged only.
        let status = git_in(&ws, &git, &["status", "--porcelain"]);
        assert_eq!(status, "M  b.txt");
        // Nothing left to commit for these paths.
        let again = git.commit_paths(&ws, &index, &["a.txt".into()], "again", &op());
        assert!(again.unwrap_err().to_string().contains("nothing to commit"));
        assert_eq!(git.head(&ws).unwrap().as_deref(), Some(c.hash.as_str()));
    }

    #[test]
    fn deletions_first_commits_and_moved_heads() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap();
        let git = Git::locate().unwrap();
        git_in(&ws, &git, &["init", "-q", "-b", "trunk"]);
        assert_eq!(git.head(&ws).unwrap(), None);
        std::fs::write(ws.join("x.txt"), "x\n").unwrap();
        let index = ws.join(".declass-index");
        let first = git
            .commit_paths(&ws, &index, &["x.txt".into()], "first", &op())
            .unwrap();
        assert_eq!(
            (first.parent, first.branch.as_deref()),
            (None, Some("trunk"))
        );
        std::fs::remove_file(ws.join("x.txt")).unwrap();
        git.commit_paths(&ws, &index, &["x.txt".into()], "remove x", &op())
            .unwrap();
        assert_eq!(
            git_in(&ws, &git, &["ls-tree", "-r", "--name-only", "HEAD"]),
            ""
        );
        // The scratch index is gone and the index matches the commit.
        assert_eq!(git_in(&ws, &git, &["status", "--porcelain"]), "");
        // HEAD moved by someone else meanwhile: the swap refuses.
        std::fs::write(ws.join("y.txt"), "y\n").unwrap();
        git_in(&ws, &git, &["add", "y.txt"]);
        git_in(&ws, &git, &["commit", "-q", "-m", "user"]);
        let user = git.head(&ws).unwrap().unwrap();
        let stale = git.run(
            &ws,
            &[
                "update-ref",
                "HEAD",
                &user,
                "0123456789012345678901234567890123456789",
            ],
            &[],
            None,
        );
        assert!(stale.is_err());
    }

    #[test]
    fn blockers_name_ignored_filtered_and_odd_paths() {
        let (_d, ws, git) = repo();
        std::fs::write(ws.join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(ws.join(".gitattributes"), "*.bin filter=lfs\n").unwrap();
        std::fs::write(ws.join("app.log"), "x").unwrap();
        std::fs::write(ws.join("blob.bin"), "x").unwrap();
        std::os::unix::fs::symlink("a.txt", ws.join("link")).unwrap();
        let b = git
            .commit_blockers(
                &ws,
                &[
                    "a.txt".into(),
                    "app.log".into(),
                    "blob.bin".into(),
                    "link".into(),
                    "gone.txt".into(),
                ],
            )
            .unwrap();
        let names: Vec<&str> = b.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(names, ["app.log", "blob.bin", "link"], "{b:?}");
        assert!(b[1].1.contains("filter `lfs`"), "{b:?}");
    }

    #[test]
    fn identity_comes_from_the_repository_then_the_owner_files() {
        let (d, ws, git) = repo();
        let owner = d.path().join("gitconfig");
        std::fs::write(
            &owner,
            "[user]\n\tname = Owner Person\n\temail = owner@example.test\n[include]\n\tpath = /nonexistent\n",
        )
        .unwrap();
        assert_eq!(git.operator_identity(&ws, &[]), None);
        let id = git.operator_identity(&ws, &[d.path().join("missing"), owner.clone()]);
        assert_eq!(id.unwrap().email, "owner@example.test");
        git_in(&ws, &git, &["config", "user.name", "Repo Person"]);
        git_in(&ws, &git, &["config", "user.email", "repo@example.test"]);
        let id = git.operator_identity(&ws, &[owner]).unwrap();
        assert_eq!(id.name, "Repo Person");
    }

    #[test]
    fn snapshots_and_diffs_work_after_a_commit() {
        let (d, ws, git) = repo();
        let store = CheckpointStore::open(git.clone(), &ws).unwrap();
        let base = git.head(&ws).unwrap().unwrap();
        let s0 = store.snapshot("runs/r/0", "start").unwrap();
        std::fs::write(ws.join("a.txt"), "two\n").unwrap();
        git.commit_paths(&ws, &d.path().join("i"), &["a.txt".into()], "a", &op())
            .unwrap();
        let s1 = store.snapshot("runs/r/1", "after commit").unwrap();
        assert_eq!(store.read(&s0, "a.txt").unwrap().unwrap(), b"one\n");
        assert_eq!(store.read(&s1, "a.txt").unwrap().unwrap(), b"two\n");
        // Against HEAD there is nothing left; against the run's base there is.
        assert!(git.diff_paths(&ws, &["a.txt".into()]).unwrap().is_empty());
        let d = git.diff_paths_from(&ws, &base, &["a.txt".into()]).unwrap();
        assert!(d.contains("-one\n+two"), "{d}");
        // The checkpoint store never touched the user's refs.
        assert_eq!(git_in(&ws, &git, &["for-each-ref", "refs/declass"]), "");
    }
}
