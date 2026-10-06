// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use declass_boundary::audit::{AuditLog, Line};
use std::sync::Mutex;

const PRIVATE: &str = "CANARY_PRIVATE_VALUE";

#[derive(Default)]
struct Filter {
    paths: Mutex<Vec<PathBuf>>,
}

impl Presenter for Filter {
    fn present(&self, source: &Source, bytes: &[u8]) -> String {
        let Source::File { path, .. } = source else {
            panic!("instructions must be classified as files")
        };
        self.paths.lock().unwrap().push(path.clone());
        String::from_utf8_lossy(bytes).replace(PRIVATE, "⟨secret:test#1⟩")
    }

    fn sanitize_message(&self, text: &str) -> String {
        text.replace(PRIVATE, "⟨secret:owner#1⟩")
    }

    fn path_visible(&self, path: &Path) -> bool {
        !path.starts_with("hidden")
    }
}

fn fixture() -> (tempfile::TempDir, RunConfig) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let workspace = root.join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(root.join("owner")).unwrap();
    let config = RunConfig {
        owner_instructions: Some(root.join("owner/DECLASS.md")),
        ..RunConfig::new(
            &workspace,
            workspace.join(".declass/runs/instructions"),
            "Task",
        )
    };
    (directory, config)
}

fn write(path: impl AsRef<Path>, text: &str) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn all_standard_root_sources_have_deterministic_order_and_named_audit_records() {
    let (directory, config) = fixture();
    let owner = config
        .owner_instructions
        .as_ref()
        .unwrap()
        .parent()
        .unwrap();
    for (name, text) in FILES.iter().zip([
        "owner generic",
        "owner claude",
        "owner gemini",
        "owner declass",
    ]) {
        write(owner.join(name), &format!("{text} {PRIVATE}"));
    }
    for (name, text) in ROOT_FILES.iter().zip([
        "project generic",
        "project claude",
        "project gemini",
        "project copilot",
        "project declass",
    ]) {
        write(config.workspace.join(name), &format!("{text} {PRIVATE}"));
    }
    write(config.workspace.join(".gitignore"), "*.md\n.github/\n");
    write(
        config.workspace.join(OVERRIDE),
        &format!("project override {PRIVATE}"),
    );
    let log = directory.path().join("audit.jsonl");
    let audit = AuditHandle::new(AuditLog::open(&log).unwrap());
    let filter = Filter::default();
    let shown = block(&config, &filter, Some(&audit)).unwrap();
    assert!(!shown.contains(PRIVATE));
    assert!(!shown.contains("project generic"));
    let order = [
        "owner generic",
        "owner claude",
        "owner gemini",
        "owner declass",
        "project override",
        "project claude",
        "project gemini",
        "project copilot",
        "project declass",
    ];
    for pair in order.windows(2) {
        assert!(
            shown.find(pair[0]).unwrap() < shown.find(pair[1]).unwrap(),
            "{pair:?}"
        );
    }
    assert!(shown.contains("operator's current request takes precedence"));
    assert!(shown.contains("These notes apply throughout the repository."));
    let origins: Vec<_> = declass_boundary::audit::read(&log)
        .unwrap()
        .into_iter()
        .filter_map(|line| match line {
            Line::Event(record) => match record.event {
                AuditEvent::Instructions { origin, sha256, .. } => {
                    assert_eq!(sha256.len(), 64);
                    Some(origin)
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        origins,
        [
            "owner:AGENTS.md",
            "owner:CLAUDE.md",
            "owner:GEMINI.md",
            "owner",
            "project:AGENTS.override.md",
            "project:CLAUDE.md",
            "project:GEMINI.md",
            "project:.github/copilot-instructions.md",
            "project"
        ]
    );
    assert!(!std::fs::read_to_string(log).unwrap().contains(PRIVATE));
    assert_eq!(filter.paths.lock().unwrap().len(), 5);
}

#[test]
fn nested_rules_follow_ancestors_only_and_overrides_replace_generic_rules() {
    let (_directory, config) = fixture();
    for (path, text) in [
        ("AGENTS.md", "root notes already loaded"),
        ("src/AGENTS.md", "outer generic"),
        ("src/DECLASS.md", "outer declass"),
        ("src/parser/AGENTS.md", "inner generic must be replaced"),
        ("src/parser/AGENTS.override.md", "inner override"),
        ("src/parser/CLAUDE.md", "inner claude"),
        ("src/parser/DECLASS.md", "inner declass"),
        ("src/other/AGENTS.md", "unrelated sibling"),
        (
            "src/parser/.github/copilot-instructions.md",
            "copilot is root only",
        ),
    ] {
        write(config.workspace.join(path), text);
    }
    let filter = Filter::default();
    let shown = scoped(&config, &filter, None, Path::new("src/parser/lib.rs")).unwrap();
    for excluded in [
        "root notes already loaded",
        "inner generic must be replaced",
        "unrelated sibling",
        "copilot is root only",
    ] {
        assert!(!shown.contains(excluded));
    }
    for pair in [
        "outer generic",
        "outer declass",
        "inner override",
        "inner claude",
        "inner declass",
    ]
    .windows(2)
    {
        assert!(shown.find(pair[0]).unwrap() < shown.find(pair[1]).unwrap());
    }
    assert_eq!(scoped(&config, &filter, None, Path::new("root.rs")), None);
}

#[test]
fn scoped_state_refreshes_changed_removed_and_newly_overridden_instructions() {
    let (_directory, config) = fixture();
    let path = config.workspace.join("src/AGENTS.md");
    write(&path, "initial rules");
    let mut state = ScopedInstructions::default();
    let filter = Filter::default();
    assert!(
        state
            .for_path(&config, &filter, None, Path::new("src/a.rs"))
            .unwrap()
            .contains("initial rules")
    );
    assert!(
        state
            .for_path(&config, &filter, None, Path::new("src/b.rs"))
            .is_none()
    );
    write(&path, "changed rules");
    assert!(
        state
            .for_path(&config, &filter, None, Path::new("src/a.rs"))
            .unwrap()
            .contains("changed rules")
    );
    let override_path = config.workspace.join("src/AGENTS.override.md");
    write(&override_path, "override rules");
    assert!(
        state
            .for_path(&config, &filter, None, Path::new("src/a.rs"))
            .unwrap()
            .contains("override rules")
    );
    std::fs::remove_file(override_path).unwrap();
    assert!(
        state
            .for_path(&config, &filter, None, Path::new("src/a.rs"))
            .unwrap()
            .contains("changed rules")
    );
    std::fs::remove_file(path).unwrap();
    assert!(
        state
            .for_path(&config, &filter, None, Path::new("src/a.rs"))
            .unwrap()
            .contains("no longer available")
    );
    assert!(
        state
            .for_path(&config, &filter, None, Path::new("src/a.rs"))
            .is_none()
    );
}

#[test]
fn traversals_hidden_paths_symlinks_and_external_includes_do_not_expose_content() {
    let (directory, config) = fixture();
    let root = directory.path().canonicalize().unwrap();
    write(root.join("outside/AGENTS.md"), "OUTSIDE_CONTENT");
    write(config.workspace.join("hidden/AGENTS.md"), "HIDDEN_CONTENT");
    write(
        config.workspace.join("AGENTS.md"),
        "must not override a refused override file",
    );
    std::os::unix::fs::symlink(
        root.join("outside/AGENTS.md"),
        config.workspace.join(OVERRIDE),
    )
    .unwrap();
    std::os::unix::fs::symlink(root.join("outside"), config.workspace.join("linked")).unwrap();
    let filter = Filter::default();
    assert!(
        block(&config, &filter, None).is_none(),
        "unreadable override cannot fall back"
    );
    let outside_path = root.join("outside/a.rs");
    for path in [
        Path::new("../outside/a.rs"),
        outside_path.as_path(),
        Path::new("hidden/a.rs"),
        Path::new(".git/config"),
        Path::new("linked/a.rs"),
    ] {
        assert!(scoped(&config, &filter, None, path).is_none(), "{path:?}");
    }
    let owner_path = config.owner_instructions.as_ref().unwrap();
    std::os::unix::fs::symlink(root.join("outside/AGENTS.md"), owner_path).unwrap();
    assert!(matches!(
        owner(owner_path),
        Some(Err(Unusable::NotAFile(_)))
    ));
    std::fs::remove_file(config.workspace.join(OVERRIDE)).unwrap();
    write(config.workspace.join("AGENTS.md"), "@../outside/AGENTS.md");
    let shown = block(&config, &filter, None).unwrap();
    assert!(shown.contains("@../outside/AGENTS.md"));
    assert!(!shown.contains("OUTSIDE_CONTENT"));
}

#[test]
fn root_and_scoped_content_are_capped_and_always_use_the_boundary() {
    let (_directory, config) = fixture();
    let large = format!("{PRIVATE} {}", "é".repeat(MAX_BYTES));
    for name in ROOT_FILES {
        write(config.workspace.join(name), &large);
    }
    for name in FILES {
        write(config.workspace.join("src").join(name), &large);
    }
    let filter = Filter::default();
    let root = block(&config, &filter, None).unwrap();
    let nested = scoped(&config, &filter, None, Path::new("src/file.rs")).unwrap();
    assert!(root.len() <= MAX_ROOT_BYTES);
    assert!(nested.len() <= MAX_SCOPED_BYTES);
    assert!(!root.contains(PRIVATE) && !nested.contains(PRIVATE));
    assert!(root.contains("context limit reached"));
    assert!(nested.contains("context limit reached"));
    assert!(
        filter
            .paths
            .lock()
            .unwrap()
            .iter()
            .any(|path| path == Path::new("src/AGENTS.md"))
    );
}
