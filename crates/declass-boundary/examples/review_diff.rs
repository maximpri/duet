// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline rule-candidate counts on recorded ordinary diffs (not a clean bill
//! of health). Usage: review_diff BEFORE AFTER [...AFTER]. No model or writes.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn sources(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut files = BTreeMap::new();
    let mut todo = vec![PathBuf::new()];
    while let Some(dir) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(root.join(&dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let rel = dir.join(entry.file_name());
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if declass_fs::is_reserved(&rel)
                || ["target", "node_modules", ".venv", "vendor"]
                    .iter()
                    .any(|s| entry.file_name() == *s)
            {
                continue;
            }
            if kind.is_dir() {
                todo.push(rel);
            } else if kind.is_file()
                && declass_review::supported(rel.extension().and_then(|s| s.to_str()).unwrap_or(""))
                && let Ok(bytes) = declass_fs::read_file(root, &rel, 256 * 1024)
                && let Ok(source) = String::from_utf8(bytes)
            {
                files.insert(rel, source);
            }
        }
    }
    files
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert!(args.len() >= 2, "expected BEFORE AFTER [...AFTER]");
    let before = sources(Path::new(&args[0]));
    for after in &args[1..] {
        let now = std::time::Instant::now();
        let (mut changed, mut candidates, mut incomplete) = (0, 0, 0);
        for (path, text) in sources(Path::new(after)) {
            if before.get(&path) == Some(&text) {
                continue;
            }
            changed += 1;
            let ext = path.extension().unwrap().to_str().unwrap();
            let prior = before
                .get(&path)
                .map(|s| declass_review::scan(ext, s, &[]))
                .unwrap_or_default();
            let diff = declass_review::introduced(prior, declass_review::scan(ext, &text, &[]));
            candidates += diff.candidates.len();
            incomplete += usize::from(diff.incomplete);
        }
        println!(
            "{}",
            serde_json::json!({"after":after,"changed_supported_files_within_cap":changed,
            "candidates":candidates,"incomplete_scans":incomplete,"seconds":now.elapsed().as_secs_f64()})
        );
    }
}
