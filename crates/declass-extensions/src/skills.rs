// SPDX-License-Identifier: GPL-3.0-or-later
//! A bounded, lazy skill catalog. Instructions are content, never permission grants.

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

pub const MAX_BYTES: u64 = 128 * 1024;
const MAX_SKILLS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Project,
    Owner,
    Plugin,
}

#[derive(Debug, Clone)]
pub struct Root {
    pub path: PathBuf,
    pub namespace: Option<String>,
    pub origin: Origin,
}

#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Canonical absolute directory containing this skill.
    pub root: PathBuf,
    /// Absolute path of this skill's instructions.
    pub path: PathBuf,
    pub origin: Origin,
    pub disable_model_invocation: bool,
    pub user_invocable: bool,
}

#[derive(Debug, Clone)]
pub struct Document {
    pub text: String,
    pub path: PathBuf,
    pub origin: Origin,
    /// Digest of these exact document bytes, including frontmatter.
    pub sha256: String,
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    skills: Vec<Skill>,
    digests: BTreeMap<String, String>,
    diagnostics: Vec<String>,
}

impl Catalog {
    /// Earlier roots win collisions. Final results are sorted by qualified ID.
    /// Missing roots are normal; malformed skills produce diagnostics.
    pub fn discover(roots: &[Root]) -> Self {
        let mut catalog = Self::default();
        for root in roots {
            if let Err(error) = catalog.discover_root(root) {
                catalog
                    .diagnostics
                    .push(safe(&format!("{}: {error:#}", root.path.display())));
            }
        }
        catalog.skills.sort_by(|a, b| a.id.cmp(&b.id));
        catalog
    }

    pub fn skills(&self) -> &[Skill] {
        &self.skills
    }

    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    /// Recheck the instruction digest before loading any resource. An edited
    /// SKILL.md requires rediscovery so metadata and instructions cannot diverge.
    pub fn load(&self, id: &str, resource: Option<&str>) -> Result<Document> {
        let index = self
            .skills
            .binary_search_by(|skill| skill.id.as_str().cmp(id))
            .map_err(|_| anyhow::anyhow!("unknown skill; refresh the skill catalog"))?;
        let skill = &self.skills[index];
        let (instructions, digest) = read_text(&skill.path)?;
        ensure!(
            self.digests.get(id) == Some(&digest),
            "skill instructions changed after discovery; refresh the skill catalog before loading"
        );
        let relative = resource.map(resource_path).transpose()?;
        let path = relative
            .as_ref()
            .map_or_else(|| skill.path.clone(), |p| skill.root.join(p));
        let (text, sha256) = if path == skill.path {
            (instructions, digest)
        } else {
            read_text(&path)?
        };
        Ok(Document {
            text,
            path,
            origin: skill.origin,
            sha256,
        })
    }

    fn discover_root(&mut self, root: &Root) -> Result<()> {
        if let Some(namespace) = &root.namespace {
            ensure!(valid_name(namespace), "invalid skill namespace");
        }
        let metadata = match std::fs::symlink_metadata(&root.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context("cannot inspect skill root"),
        };
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "skill root must be a directory, not a symlink"
        );
        // Root locations are selected by the host. Resolve aliases in their
        // parents once, then open every content path from / without links.
        let canonical = root
            .path
            .canonicalize()
            .context("cannot resolve skill root")?;
        let mut folders = BTreeSet::new();
        let mut capped = false;
        for entry in std::fs::read_dir(&canonical).context("cannot list skill root")? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            folders.insert(name);
            if folders.len() > MAX_SKILLS {
                folders.pop_last();
                capped = true;
            }
        }
        if capped {
            self.diagnostics.push(safe(&format!(
                "{}: discovery limited to {MAX_SKILLS} skill directories",
                canonical.display()
            )));
        }
        for folder in folders {
            let directory = canonical.join(&folder);
            let path = directory.join("SKILL.md");
            let result = self.discover_skill(root, &folder, directory, path.clone());
            if let Err(error) = result {
                self.diagnostics
                    .push(safe(&format!("{}: {error:#}", path.display())));
            }
        }
        Ok(())
    }

    fn discover_skill(
        &mut self,
        root: &Root,
        folder: &str,
        directory: PathBuf,
        path: PathBuf,
    ) -> Result<()> {
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context("cannot inspect skill instructions"),
        };
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "skill instructions must be a regular file, not a symlink"
        );
        ensure!(valid_name(folder), "invalid skill directory name");
        let (text, digest) = read_text(&path)?;
        let fields = frontmatter(&text)?;
        ensure!(fields.name == folder, "skill name must match its directory");
        let id = root.namespace.as_ref().map_or_else(
            || fields.name.clone(),
            |namespace| format!("{namespace}:{}", fields.name),
        );
        if self.digests.contains_key(&id) {
            self.diagnostics.push(format!(
                "Skill {id} appears in more than one root; kept the earlier root."
            ));
            return Ok(());
        }
        if self.skills.len() == MAX_SKILLS {
            bail!("catalog reached its {MAX_SKILLS}-skill limit");
        }
        self.digests.insert(id.clone(), digest);
        self.skills.push(Skill {
            id,
            name: fields.name,
            description: fields.description,
            root: directory,
            path,
            origin: root.origin,
            disable_model_invocation: fields.disable_model_invocation,
            user_invocable: fields.user_invocable,
        });
        Ok(())
    }
}

fn safe(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn resource_path(resource: &str) -> Result<PathBuf> {
    ensure!(
        !resource.is_empty()
            && resource.len() <= 4096
            && !resource.contains('\\')
            && !resource.chars().any(char::is_control)
            && resource
                .split('/')
                .all(|part| !matches!(part, "" | "." | "..")),
        "resource must be a relative path inside the skill directory"
    );
    let path = Path::new(resource);
    ensure!(
        path.components()
            .all(|part| matches!(part, Component::Normal(_))),
        "resource must be a relative path inside the skill directory"
    );
    Ok(path.to_owned())
}

fn read_text(path: &Path) -> Result<(String, String)> {
    let relative = path
        .strip_prefix(Path::new("/"))
        .context("skill path must be absolute")?;
    let bytes = declass_fs::read_file(Path::new("/"), relative, MAX_BYTES)
        .context("cannot read skill resource safely (128 KiB limit)")?;
    ensure!(!bytes.contains(&0), "skill resource is not a text file");
    let digest = hex::encode(Sha256::digest(&bytes));
    let text = String::from_utf8(bytes).context("skill resource must be UTF-8 text")?;
    Ok((text, digest))
}

struct Fields {
    name: String,
    description: String,
    disable_model_invocation: bool,
    user_invocable: bool,
}

/// Parse only the portable fields the host uses. Extra fields and nested
/// metadata stay inert; anchors, tags and collections cannot become a name,
/// description or invocation flag. The full original document is loaded later.
fn frontmatter(text: &str) -> Result<Fields> {
    let lines: Vec<_> = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .lines()
        .collect();
    ensure!(
        lines.first().is_some_and(|line| *line == "---"),
        "SKILL.md must start with YAML frontmatter"
    );
    let end = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.trim_end() == "---")
        .map(|(index, _)| index)
        .context("YAML frontmatter has no closing ---")?;
    let mut fields = BTreeMap::new();
    let mut index = 1;
    while index < end {
        let line = lines[index];
        index += 1;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        ensure!(
            !line.starts_with(char::is_whitespace),
            "unsupported YAML indentation"
        );
        let (key, value) = line.split_once(':').context("expected a YAML field")?;
        ensure!(
            !key.is_empty()
                && key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')),
            "unsupported YAML field name"
        );
        ensure!(!fields.contains_key(key), "duplicate YAML field {key}");
        let first = index;
        while index < end && (lines[index].trim().is_empty() || lines[index].starts_with(' ')) {
            index += 1;
        }
        fields.insert(key, (value.trim(), &lines[first..index]));
    }
    let required = |key: &str| -> Result<String> {
        let (value, continuation) = fields
            .get(key)
            .with_context(|| format!("missing skill {key}"))?;
        scalar(value, continuation, key == "description")
            .with_context(|| format!("invalid skill {key}"))
    };
    let name = required("name")?;
    ensure!(
        valid_name(&name),
        "skill name must contain 1–64 lowercase letters, digits or single hyphens"
    );
    let description = required("description")?;
    ensure!(
        !description.is_empty() && description.chars().count() <= 1024,
        "skill description must contain 1–1024 characters"
    );
    ensure!(
        !description
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t')),
        "control characters are not allowed in skill metadata"
    );
    let boolean = |key: &str, default: bool| -> Result<bool> {
        let Some((value, continuation)) = fields.get(key) else {
            return Ok(default);
        };
        ensure!(
            continuation.iter().all(|line| line.trim().is_empty()),
            "unsupported YAML boolean continuation"
        );
        match plain(value) {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => bail!("{key} must be true or false"),
        }
    };
    Ok(Fields {
        name,
        description,
        disable_model_invocation: boolean("disable-model-invocation", false)?,
        user_invocable: boolean("user-invocable", true)?,
    })
}

fn scalar(value: &str, continuation: &[&str], allow_block: bool) -> Result<String> {
    if value.starts_with(['|', '>']) {
        ensure!(
            allow_block,
            "block syntax is only supported for descriptions"
        );
        return block(value, continuation);
    }
    ensure!(
        continuation.iter().all(|line| line.trim().is_empty()),
        "use > or | for a multiline description"
    );
    let value = if value.starts_with('"') {
        let mut escape = false;
        let mut end = None;
        for (index, c) in value.char_indices().skip(1) {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                end = Some(index + 1);
                break;
            }
        }
        let end = end.context("unterminated double-quoted scalar")?;
        tail(&value[end..])?;
        serde_json::from_str::<String>(&value[..end]).context("unsupported quoted YAML escape")?
    } else if let Some(rest) = value.strip_prefix('\'') {
        let mut output = String::new();
        let mut chars = rest.char_indices().peekable();
        let mut closed = false;
        while let Some((index, c)) = chars.next() {
            if c != '\'' {
                output.push(c);
                continue;
            }
            if chars.peek().is_some_and(|(_, c)| *c == '\'') {
                chars.next();
                output.push('\'');
            } else {
                tail(&rest[index + 1..])?;
                closed = true;
                break;
            }
        }
        ensure!(closed, "unterminated single-quoted scalar");
        output
    } else {
        let value = plain(value);
        ensure!(
            !value.starts_with(['[', '{', '&', '*', '!', '?', '\u{0060}', '@'])
                && !value.starts_with("- ")
                && !value.contains(": "),
            "unsupported YAML scalar syntax"
        );
        value.to_owned()
    };
    ensure!(
        !value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t')),
        "control characters are not allowed in skill metadata"
    );
    Ok(value.trim().to_owned())
}

fn tail(value: &str) -> Result<()> {
    ensure!(
        value.trim().is_empty() || value.trim_start().starts_with('#'),
        "unexpected text after a quoted scalar"
    );
    Ok(())
}

fn plain(value: &str) -> &str {
    let mut previous_space = true;
    for (index, c) in value.char_indices() {
        if c == '#' && previous_space {
            return value[..index].trim_end();
        }
        previous_space = c.is_whitespace();
    }
    value.trim_end()
}

fn block(indicator: &str, lines: &[&str]) -> Result<String> {
    let indicator = plain(indicator);
    let folded = indicator.starts_with('>');
    let mut indentation = None;
    let mut chomp = None;
    for c in indicator.chars().skip(1) {
        match c {
            '1'..='9' if indentation.is_none() => indentation = c.to_digit(10).map(|n| n as usize),
            '+' | '-' if chomp.is_none() => chomp = Some(c),
            _ => bail!("unsupported YAML block indicator"),
        }
    }
    let indent = indentation
        .or_else(|| {
            lines
                .iter()
                .find(|line| !line.trim().is_empty())
                .map(|line| line.len() - line.trim_start_matches(' ').len())
        })
        .unwrap_or(0);
    ensure!(indent > 0, "a block description must be indented");
    let mut content = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            content.push("");
            continue;
        }
        ensure!(
            line.as_bytes().iter().take(indent).all(|b| *b == b' ') && line.len() >= indent,
            "inconsistent block description indentation"
        );
        content.push(&line[indent..]);
    }
    let mut output = String::new();
    for (index, line) in content.iter().enumerate() {
        output.push_str(line);
        if let Some(next) = content.get(index + 1) {
            if !folded || line.starts_with(' ') || next.starts_with(' ') {
                output.push('\n');
            } else if !line.is_empty() && !next.is_empty() {
                output.push(' ');
            } else if !line.is_empty() || next.is_empty() || output.is_empty() {
                output.push('\n');
            }
        }
    }
    // Descriptions are metadata summaries: trailing chomping differences are
    // intentionally trimmed. Instruction bodies retain their exact bytes.
    Ok(output.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn root(dir: &TempDir, namespace: Option<&str>, origin: Origin) -> Root {
        Root {
            path: dir.path().to_owned(),
            namespace: namespace.map(str::to_owned),
            origin,
        }
    }

    fn put(dir: &TempDir, name: &str, text: &str) -> PathBuf {
        let folder = dir.path().join(name);
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("SKILL.md");
        fs::write(&path, text).unwrap();
        path
    }

    fn basic(name: &str, description: &str) -> String {
        format!(
            "---\nname: {name}\ndescription: {description}\n---\n# Instructions\nRead the supporting notes.\n"
        )
    }

    #[test]
    fn quoted_metadata_flags_and_inert_fields_preserve_original_instructions() {
        let dir = TempDir::new().unwrap();
        let text = "---\nname: 'review-code'\ndescription: \"Review \\\"code\\\" carefully # safely\" # comment\ndisable-model-invocation: true # manual only\nuser-invocable: false\nlicense: MIT\ncompatibility: Needs a terminal\nallowed-tools: shell(*), network(*)\nmetadata:\n  author: example\n  nested:\n    value: [one, two]\n---\nDo not execute anything at discovery.\n";
        put(&dir, "review-code", text);
        let catalog = Catalog::discover(&[root(&dir, None, Origin::Owner)]);
        assert!(
            catalog.diagnostics().is_empty(),
            "{:?}",
            catalog.diagnostics()
        );
        let skill = &catalog.skills()[0];
        assert_eq!(skill.description, "Review \"code\" carefully # safely");
        assert!(skill.disable_model_invocation);
        assert!(!skill.user_invocable);
        assert!(skill.path.is_absolute());
        assert_eq!(skill.path, skill.root.join("SKILL.md"));
        let document = catalog.load("review-code", None).unwrap();
        assert_eq!(document.text, text);
        assert_eq!(document.origin, Origin::Owner);
        assert_eq!(
            document.sha256,
            hex::encode(Sha256::digest(text.as_bytes()))
        );
        assert_eq!(
            catalog.load("review-code", Some("SKILL.md")).unwrap().text,
            text
        );
        let single =
            frontmatter("---\nname: review-code\ndescription: 'It''s # literal' # comment\n---")
                .unwrap();
        assert_eq!(single.description, "It's # literal");
        assert!(!single.disable_model_invocation);
        assert!(single.user_invocable);
    }

    #[test]
    fn folded_and_literal_descriptions_support_paragraphs_unicode_and_indentation() {
        let folded = frontmatter("---\nname: review\ndescription: >- # summary\n  Review café\n  and code.\n\n  Keep context.\n---").unwrap();
        assert_eq!(folded.description, "Review café and code.\nKeep context.");
        let literal = frontmatter("---\nname: review\ndescription: |2+\n  First line.\n    Preserve indentation.\n  Last line.\n\n---").unwrap();
        assert_eq!(
            literal.description,
            "First line.\n  Preserve indentation.\nLast line."
        );
        assert!(
            frontmatter("---\nname: review\ndescription: |\n  Unsafe \u{1b}[31m\n---").is_err()
        );
    }

    #[test]
    fn malformed_required_metadata_fails_with_diagnostics() {
        let invalid = [
            "name: bad\n",
            "---\nname: bad\ndescription: missing close\n",
            "---\nname: bad\n---\n",
            "---\nname: bad\ndescription: one\ndescription: two\n---\n",
            "---\nname: [bad]\ndescription: text\n---\n",
            "---\nname: bad\ndescription: &anchor text\n---\n",
            "---\nname: bad\ndescription: *anchor\n---\n",
            "---\nname: bad\ndescription: [one, two]\n---\n",
            "---\nname: bad\ndescription: \"unterminated\n---\n",
            "---\nname: bad\ndescription: text\ndisable-model-invocation: yes\n---\n",
            "---\nname: bad\ndescription: text\nuser-invocable: \"false\"\n---\n",
            "---\nname: bad\ndescription: >\n  one\n other indent\n---\n",
            "---\nname: bad\ndescription: plain\n  continued\n---\n",
        ];
        let dir = TempDir::new().unwrap();
        for text in invalid {
            put(&dir, "bad", text);
            let catalog = Catalog::discover(&[root(&dir, None, Origin::Project)]);
            assert!(catalog.skills().is_empty(), "accepted {text:?}");
            assert_eq!(catalog.diagnostics().len(), 1);
        }
        for name in ["", "-bad", "bad-", "bad--name", "Bad", "café", "a/b"] {
            assert!(!valid_name(name), "accepted {name:?}");
        }
        assert!(valid_name(&"a".repeat(64)));
        assert!(!valid_name(&"a".repeat(65)));
        assert!(frontmatter(&basic("bad", &"é".repeat(1024))).is_ok());
        assert!(frontmatter(&basic("bad", &"é".repeat(1025))).is_err());
        put(&dir, "bad", &basic("different-name", "wrong directory"));
        assert!(
            Catalog::discover(&[root(&dir, None, Origin::Project)])
                .skills()
                .is_empty()
        );
    }

    #[test]
    fn deterministic_precedence_namespaces_and_missing_roots() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        put(&first, "zebra", &basic("zebra", "Last alphabetically"));
        put(&first, "same", &basic("same", "Project wins"));
        put(&second, "same", &basic("same", "Owner fallback"));
        put(&second, "alpha", &basic("alpha", "First alphabetically"));
        let roots = [
            root(&first, None, Origin::Project),
            root(&second, None, Origin::Owner),
            root(&second, Some("demo"), Origin::Plugin),
            Root {
                path: first.path().join("absent"),
                namespace: None,
                origin: Origin::Project,
            },
        ];
        let catalog = Catalog::discover(&roots);
        assert_eq!(
            catalog
                .skills()
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "demo:alpha", "demo:same", "same", "zebra"]
        );
        assert_eq!(catalog.diagnostics().len(), 1);
        assert!(catalog.diagnostics()[0].contains("kept the earlier root"));
        assert!(
            catalog
                .load("same", None)
                .unwrap()
                .text
                .contains("Project wins")
        );
        assert_eq!(
            catalog.load("demo:same", None).unwrap().origin,
            Origin::Plugin
        );
        assert!(
            Catalog::discover(&[root(&second, Some("../evil"), Origin::Plugin)])
                .skills()
                .is_empty()
        );
    }

    #[test]
    fn changed_instructions_require_refresh_for_body_and_resources_including_clones() {
        let dir = TempDir::new().unwrap();
        let path = put(&dir, "review", &basic("review", "Before"));
        let notes = path.parent().unwrap().join("notes.txt");
        fs::write(&notes, "supporting notes").unwrap();
        let catalog = Catalog::discover(&[root(&dir, None, Origin::Project)]);
        let clone = catalog.clone();
        let resource = catalog.load("review", Some("notes.txt")).unwrap();
        assert_eq!(resource.text, "supporting notes");
        assert_eq!(resource.path.file_name().unwrap(), "notes.txt");
        assert_eq!(
            resource.sha256,
            hex::encode(Sha256::digest(b"supporting notes"))
        );
        fs::write(path, basic("review", "After")).unwrap();
        for snapshot in [&catalog, &clone] {
            for resource in [None, Some("notes.txt")] {
                assert!(
                    snapshot
                        .load("review", resource)
                        .unwrap_err()
                        .to_string()
                        .contains("refresh")
                );
            }
        }
        let refreshed = Catalog::discover(&[root(&dir, None, Origin::Project)]);
        assert_eq!(refreshed.skills()[0].description, "After");
        assert!(refreshed.load("review", None).is_ok());
        assert!(refreshed.load("unknown", None).is_err());
    }

    #[test]
    fn bounded_utf8_text_only_and_path_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        let path = put(&dir, "review", &basic("review", "Description"));
        let catalog = Catalog::discover(&[root(&dir, None, Origin::Project)]);
        let folder = path.parent().unwrap();
        fs::write(folder.join("exact.txt"), vec![b'x'; MAX_BYTES as usize]).unwrap();
        assert_eq!(
            catalog
                .load("review", Some("exact.txt"))
                .unwrap()
                .text
                .len(),
            MAX_BYTES as usize
        );
        fs::write(folder.join("big.txt"), vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        fs::write(folder.join("nul.txt"), b"a\0b").unwrap();
        fs::write(folder.join("binary.txt"), [0xff]).unwrap();
        for resource in [
            "big.txt",
            "nul.txt",
            "binary.txt",
            "",
            ".",
            "..",
            "../SKILL.md",
            "/etc/passwd",
            "notes/../SKILL.md",
            "./SKILL.md",
            "notes//file",
            "notes\\file",
        ] {
            assert!(
                catalog.load("review", Some(resource)).is_err(),
                "accepted {resource:?}"
            );
        }
        fs::create_dir(folder.join("nested")).unwrap();
        fs::write(folder.join("nested/notes.md"), "Nested text").unwrap();
        assert_eq!(
            catalog
                .load("review", Some("nested/notes.md"))
                .unwrap()
                .text,
            "Nested text"
        );
        fs::write(path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        assert!(
            Catalog::discover(&[root(&dir, None, Origin::Project)])
                .skills()
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_at_root_skill_file_and_resource_directories_are_rejected() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = put(&outside, "review", &basic("review", "External"));
        symlink(outside.path(), dir.path().join("linked-root")).unwrap();
        let catalog = Catalog::discover(&[Root {
            path: dir.path().join("linked-root"),
            namespace: None,
            origin: Origin::Project,
        }]);
        assert!(catalog.skills().is_empty());
        assert_eq!(catalog.diagnostics().len(), 1);
        symlink(target.parent().unwrap(), dir.path().join("review")).unwrap();
        assert!(
            Catalog::discover(&[root(&dir, None, Origin::Project)])
                .skills()
                .is_empty()
        );
        fs::remove_file(dir.path().join("review")).unwrap();
        fs::create_dir(dir.path().join("review")).unwrap();
        symlink(&target, dir.path().join("review/SKILL.md")).unwrap();
        assert!(
            Catalog::discover(&[root(&dir, None, Origin::Project)])
                .skills()
                .is_empty()
        );
        fs::remove_file(dir.path().join("review/SKILL.md")).unwrap();
        put(&dir, "review", &basic("review", "Local"));
        let catalog = Catalog::discover(&[root(&dir, None, Origin::Project)]);
        symlink(&target, dir.path().join("review/link.md")).unwrap();
        symlink(target.parent().unwrap(), dir.path().join("review/nested")).unwrap();
        assert!(catalog.load("review", Some("link.md")).is_err());
        assert!(catalog.load("review", Some("nested/SKILL.md")).is_err());
        // Even matching instruction bytes cannot authorize a folder redirected
        // after discovery: every path component is opened without following links.
        fs::write(&target, basic("review", "Local")).unwrap();
        fs::rename(dir.path().join("review"), dir.path().join("previous")).unwrap();
        symlink(target.parent().unwrap(), dir.path().join("review")).unwrap();
        assert!(catalog.load("review", None).is_err());
    }
}
