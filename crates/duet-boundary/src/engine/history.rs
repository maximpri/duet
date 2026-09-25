// SPDX-License-Identifier: GPL-3.0-or-later
//! Git history in the engine (`Source::GitHistory`).
//!
//! A path's history is classified by the path as it is now, in every
//! revision: a sensitive path's old content and diffs are held locally like
//! `read_file` holds the file (a secret file is shown with its values
//! replaced); a protected path's history is withheld; a public path's history
//! is scanned, since old commits often hold keys removed since. Commit
//! metadata (messages, author names and emails, status lines) is scanned the
//! same way: an email becomes a placeholder.

use super::Engine;
use crate::bulky::Shape;
use crate::policy::{IpLevel, is_secret_bearing};
use crate::view::ViewClass;
use std::path::Path;

impl Engine {
    pub(super) fn history_view(&self, rev: &str, path: Option<&Path>, text: &str) -> String {
        let Some(path) = path else {
            if self.offload(text) {
                return self.bulky_view(&format!("git history at {rev}"), text, Shape::Output);
            }
            let mut st = self.lock();
            return self.clean_public(&mut st, text, "git history");
        };
        let p = path.display();
        let label = format!("{p} at {rev}");
        match self.policy.ip_level(path) {
            Some(IpLevel::Sealed) => {
                self.set_class(ViewClass::Protected);
                return format!("[{p} is sealed: its history is never shown]\n");
            }
            Some(IpLevel::InterfaceOnly) => {
                self.set_class(ViewClass::Protected);
                // Old bodies are protected code too: copies stay caught.
                self.lock().overlap.add_sensitive(text);
                return format!(
                    "[{p} is interface-only: its content and changes at {rev} are withheld; read_file shows \
its current interface]\n"
                );
            }
            None => {}
        }
        if self.is_sensitive(path) {
            // Only the file's own lines count as its content: git's headers
            // (hashes, modes, hunk ranges) and blame's commit lines are not.
            let (marks, bodies) = split_content(text);
            let body = bodies.join("\n");
            // Old values were never primed: register them, so an echo of them
            // anywhere later (and any outbound request) is caught.
            {
                let mut st = self.lock();
                let _ = self.sanitize(&mut st, &body, &label, true);
            }
            if is_secret_bearing(path) {
                self.set_class(ViewClass::Tokenized);
                let mut st = self.lock();
                st.overlap.add_sensitive(&body);
                let view = self.tokenized_view(&mut st, &label, &body);
                let mut lines = view.lines();
                let mut out = lines.next().unwrap_or_default().to_owned() + "\n";
                for mark in marks {
                    out.push_str(mark);
                    out.push_str(lines.next().unwrap_or_default());
                    out.push('\n');
                }
                return out;
            }
            return self.handle_view(&format!("history of {label}"), text);
        }
        if self.offload(text) {
            return self.bulky_view(&label, text, Shape::Output);
        }
        let mut st = self.lock();
        self.clean_public(&mut st, text, &label)
    }
}

/// Lines of git's own that a history text holds outside diff hunks (never
/// the file's content): diff headers and blame's `commit` lines.
const GIT_LINES: &[&str] = &[
    "diff --git ",
    "index ",
    "new file mode ",
    "deleted file mode ",
    "old mode ",
    "new mode ",
    "similarity index ",
    "rename from ",
    "rename to ",
    "--- ",
    "+++ ",
    "Binary files ",
    "commit ",
];

/// Each line of a history text as (git's part, the file's content). Inside a
/// hunk the `+`/`-`/space mark is git's; a hunk header is git's up to its
/// closing `@@` (the function context after it is content); a numbered line
/// (`12  text`) is content after its number; outside hunks only the lines in
/// [`GIT_LINES`] are git's. Anything else is content, so an unexpected shape
/// is never shown raw.
fn split_content(text: &str) -> (Vec<&str>, Vec<&str>) {
    let (mut marks, mut bodies) = (Vec::new(), Vec::new());
    let mut in_hunk = false;
    for line in text.lines() {
        if line.starts_with("diff --git ") {
            in_hunk = false;
        }
        let split = if let Some(rest) = line.strip_prefix("@@") {
            in_hunk = true;
            rest.find("@@").map_or(line.len(), |i| i + 4)
        } else if in_hunk && line.starts_with(['+', '-', ' ', '\\']) {
            if line.starts_with('\\') {
                line.len()
            } else {
                1
            }
        } else if !in_hunk && GIT_LINES.iter().any(|g| line.starts_with(g)) {
            line.len()
        } else {
            let lead = line.len() - line.trim_start().len();
            let digits = line[lead..].bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0 && line[lead + digits..].starts_with("  ") {
                lead + digits + 2
            } else {
                0
            }
        };
        let (mark, body) = line.split_at(split.min(line.len()));
        marks.push(mark);
        bodies.push(body);
    }
    (marks, bodies)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Policy;
    use crate::view::{Presenter, Source};
    use std::path::PathBuf;

    const KEY: &str = "sk_live_Qm8vT2xW9pL4nR7kZ3cY6bH1";
    const EMAIL: &str = "amelia.velanwick42@mailbox-311.net";
    const BALANCE: &str = "8977066";

    fn engine() -> (tempfile::TempDir, std::sync::Arc<Engine>) {
        let d = tempfile::tempdir().unwrap();
        let policy = Policy {
            sensitive_globs: vec![".env*".into(), "data/**".into()],
            command_output_sensitive: true,
            detect_secrets: true,
            detect_pii: true,
            detect_entropy: true,
            bulky_tokens: 2000,
            bulky_file_tokens: 2000,
            interface_only: vec!["src/pricing.rs".into()],
            sealed: vec!["src/secret_sauce.rs".into()],
            ..Policy::default()
        };
        let e = Engine::open(d.path(), policy, None).unwrap();
        (d, e)
    }

    fn at(path: Option<&str>) -> Source {
        Source::GitHistory {
            rev: "abc1234".into(),
            path: path.map(PathBuf::from),
        }
    }

    #[test]
    fn a_sensitive_path_is_sensitive_in_every_revision() {
        let (_d, e) = engine();
        let old = format!("id,total\n1,{BALANCE}\n");
        let shown = e.present(&at(Some("data/orders.csv")), old.as_bytes());
        assert!(!shown.contains(BALANCE), "{shown}");
        assert!(shown.contains("ask_local"), "{shown}");
        assert_eq!(e.take_view_class(), Some(ViewClass::HandleSummary));
        let env = format!("-PAYMENTS_KEY={KEY}\n+PAYMENTS_KEY=\n");
        let shown = e.present(&at(Some(".env")), env.as_bytes());
        assert!(
            !shown.contains(KEY) && shown.contains("PAYMENTS_KEY"),
            "{shown}"
        );
        // Copies of the old content are caught elsewhere too.
        let echo = e.present(&at(None), BALANCE.as_bytes());
        assert!(!echo.contains(BALANCE), "{echo}");
    }

    #[test]
    fn git_headers_are_not_content() {
        let (_d, e) = engine();
        let diff = format!(
            "diff --git a/data/o.csv b/data/o.csv\nnew file mode 100644\nindex 0000000..7dbd4a6\n--- /dev/null\n\
+++ b/data/o.csv\n@@ -0,0 +1,2 @@\n+id,total\n+1,{BALANCE}\n"
        );
        e.present(&at(Some("data/o.csv")), diff.as_bytes());
        let blame = format!("commit 499278f894 2026-09-25 Amelia Velanwick\n  1  1,{BALANCE}\n");
        e.present(&at(Some("data/o.csv")), blame.as_bytes());
        // The file's values are known now; git's hashes and modes are not.
        let public = format!("index 0000000..7dbd4a6 100644 499278f894 {BALANCE}\n");
        let shown = e.present(&at(Some("src/x.rs")), public.as_bytes());
        assert!(
            shown.starts_with("index 0000000..7dbd4a6 100644 499278f894 ")
                && !shown.contains(BALANCE),
            "{shown}"
        );
        let env = e.present(
            &at(Some(".env")),
            format!("@@ -1,2 +1,2 @@ OTHER={KEY}x\n-PAY_KEY={KEY}\n+PAY_KEY=\n").as_bytes(),
        );
        assert!(
            env.contains("\n-PAY_KEY=⟨") && env.contains("\n+PAY_KEY="),
            "{env}"
        );
        assert!(
            env.contains("@@ -1,2 +1,2 @@ OTHER=⟨") && !env.contains(KEY),
            "{env}"
        );
    }

    #[test]
    fn public_history_and_metadata_are_scanned() {
        let (_d, e) = engine();
        let diff = format!(
            "diff --git a/src/pay.rs b/src/pay.rs\n-const KEY: &str = \"{KEY}\";\n+let key = std::env::var(\"KEY\")?;\n"
        );
        let shown = e.present(&at(Some("src/pay.rs")), diff.as_bytes());
        assert!(!shown.contains(KEY), "{shown}");
        assert!(shown.contains("std::env::var"), "{shown}");
        let log = format!("abc1234 2026-09-01 Amelia Velanwick <{EMAIL}> Remove key\n");
        let shown = e.present(&at(None), log.as_bytes());
        assert!(
            !shown.contains(EMAIL) && shown.contains("Remove key"),
            "{shown}"
        );
    }

    #[test]
    fn protected_history_is_withheld() {
        let (_d, e) = engine();
        let body = b"fn discount() -> u32 { 917 }\n";
        let sealed = e.present(&at(Some("src/secret_sauce.rs")), body);
        assert!(
            !sealed.contains("917") && sealed.contains("sealed"),
            "{sealed}"
        );
        let iface = e.present(&at(Some("src/pricing.rs")), body);
        assert!(
            !iface.contains("917") && iface.contains("withheld"),
            "{iface}"
        );
        assert_eq!(e.take_view_class(), Some(ViewClass::Protected));
        assert_eq!(
            e.protection(Path::new("src/pricing.rs")),
            Some(IpLevel::InterfaceOnly)
        );
    }
}
