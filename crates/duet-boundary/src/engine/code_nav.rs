// SPDX-License-Identifier: GPL-3.0-or-later
//! Language-server answers ([`Source::CodeNav`]), classified by the file they
//! come from exactly as `read_file` would show it:
//!
//! - sensitive (by policy, or derived during the run): nothing of the content;
//! - sealed: nothing of the content;
//! - interface-only: declarations (hover text, symbol names) with any line
//!   of a withheld body replaced; never a line of the file itself, which may
//!   lie inside a withheld body;
//! - anything else: the text, sanitized like an open file.
//!
//! The caller shows only locations (path, line, column) next to these views.

use super::Engine;
use crate::policy::IpLevel;
use crate::view::{CODE_NAV_WITHHELD, ViewClass};
use std::path::Path;

impl Engine {
    pub(super) fn code_nav_view(&self, path: &Path, signature: bool, text: &str) -> String {
        if self.is_sensitive(path) {
            self.set_class(ViewClass::HandleSummary);
            return format!("{CODE_NAV_WITHHELD} sensitive file⟩");
        }
        let label = path.display().to_string();
        match self.policy.ip_level(path) {
            Some(IpLevel::Sealed) => {
                self.set_class(ViewClass::Protected);
                format!("{CODE_NAV_WITHHELD} sealed source⟩")
            }
            Some(IpLevel::InterfaceOnly) if !signature => {
                self.set_class(ViewClass::Protected);
                format!("{CODE_NAV_WITHHELD} protected code⟩")
            }
            Some(IpLevel::InterfaceOnly) => {
                self.set_class(ViewClass::Protected);
                let mut st = self.lock();
                let clean = self.sanitize(&mut st, text, &label, false);
                Self::ip_redact(&mut st, &clean).0
            }
            None => {
                let mut st = self.lock();
                self.sanitize(&mut st, text, &label, false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use crate::policy::Policy;
    use crate::view::{CODE_NAV_WITHHELD, Presenter, Source, ViewClass};
    use std::path::PathBuf;

    const KEY: &str = "sk_live_Qm8vT2xW9pL4nR7kZ3cY6bH1";

    fn nav(path: &str, signature: bool) -> Source {
        Source::CodeNav {
            path: PathBuf::from(path),
            signature,
        }
    }

    #[test]
    fn answers_are_shown_as_their_file_would_be() {
        let d = tempfile::tempdir().unwrap();
        let policy = Policy {
            sensitive_globs: vec!["data/**".into()],
            interface_only: vec!["src/pricing/**".into()],
            sealed: vec!["src/vault/**".into()],
            detect_secrets: true,
            ..Policy::default()
        };
        let e = Engine::open(d.path(), policy, None).unwrap();
        let cases = [
            (
                nav("data/customers.rs", true),
                "fn amelia_velanwick() -> u64 { 8977066 }",
                ViewClass::HandleSummary,
            ),
            (
                nav("src/vault/keys.rs", true),
                "fn master_key() -> &'static str",
                ViewClass::Protected,
            ),
            (
                nav("src/pricing/engine.rs", false),
                "let uplift = base_rate * 0.8731;",
                ViewClass::Protected,
            ),
        ];
        for (source, text, class) in cases {
            let shown = e.present(&source, text.as_bytes());
            assert!(shown.starts_with(CODE_NAV_WITHHELD), "{source:?}: {shown}");
            assert!(!shown.contains(text), "{shown}");
            assert_eq!(e.take_view_class(), Some(class), "{source:?}");
        }
        // Declarations of an interface-only file are shown.
        let sig = "pub fn quote(base_cents: u64) -> u64";
        assert_eq!(
            e.present(&nav("src/pricing/engine.rs", true), sig.as_bytes()),
            sig
        );
        assert_eq!(e.take_view_class(), Some(ViewClass::Protected));
        // An open file's line is shown sanitized.
        let line = format!("const KEY: &str = \"{KEY}\";");
        let shown = e.present(&nav("src/main.rs", false), line.as_bytes());
        assert!(
            shown.starts_with("const KEY") && !shown.contains(KEY),
            "{shown}"
        );
        assert_eq!(e.take_view_class(), Some(ViewClass::Raw));
    }
}
