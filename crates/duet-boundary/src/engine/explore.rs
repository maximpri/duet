// SPDX-License-Identifier: GPL-3.0-or-later
//! The local explorer's report ([`Source::Explore`](crate::view::Source)).
//!
//! The explorer is the local model reading the workspace as it is, sensitive
//! and protected files included, to answer one question of the frontier's.
//! Its report is local-model output about everything it read, so it is
//! cleaned like an `ask_local` answer about that text
//! ([`Engine::clean_local_counted`]: copied runs, spelled-out and encoded
//! values, detected and known values, digits of withheld numbers, and the
//! per-value budget, with a positional question put as one about format),
//! and then loses every line quoting protected code. What it read joins the
//! indexes first: sensitive text as sensitive (a file the run-start index
//! did not take, or one written since), protected source through its own
//! index, and public files as public (code they share is never taken for a
//! quotation, and their words are never taken for names).

use super::{Answered, Engine, words};
use crate::audit::AuditEvent;
use crate::probing;
use crate::view::{Explored, ViewClass};

/// The handle name the explorer's probes are counted under.
const PROBES: &str = "explore";

impl Engine {
    pub(super) fn explore_view(&self, question: &str, read: &Explored, text: &str) -> String {
        self.set_class(ViewClass::LocalAnswer);
        let mut st = self.lock();
        for (path, t) in read.0.iter() {
            let Some(path) = path else { continue };
            let digest = duet_fs::sha256_hex(format!("{}\0{t}", path.display()).as_bytes());
            if !st.explored.insert(digest) {
                continue;
            }
            if let Some(level) = self.policy.ip_level(path) {
                let whole = self.ip_full_text(&st, path, t);
                self.ip_index(&mut st, path, level, &whole);
            } else if self.is_sensitive(path) {
                self.index_sensitive(&mut st, path, t);
            } else {
                st.overlap.add_public(t);
                st.public_words.extend(words(t));
            }
        }
        let all: String = read
            .0
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let question = st.vault.detokenize(question).0;
        let narrow = probing::positional_question(&question);
        let answered = Answered {
            question: &question,
            narrow,
            evidence: &[],
        };
        let (clean, withheld) =
            self.clean_local_counted(&mut st, text, "explorer report", &all, Some(answered));
        let (clean, _) = Self::ip_redact(&mut st, &clean);
        if narrow || withheld > 0 {
            let count = st.probes.probe(PROBES);
            let _ = duet_fs::private::write_private(
                &self.probes_file,
                &serde_json::to_vec(&st.probes).unwrap_or_default(),
            );
            self.record(AuditEvent::LocalProbe {
                handle: PROBES.into(),
                rule: if narrow {
                    "positional_question"
                } else {
                    "characters_withheld"
                }
                .into(),
                withheld: withheld as u32,
                count,
            });
        }
        clean
    }
}

#[cfg(test)]
mod tests {
    use crate::audit::AuditEvent;
    use crate::engine::Engine;
    use crate::policy::Policy;
    use crate::view::{Explored, Presenter, Source, ViewClass};
    use std::path::PathBuf;
    use std::sync::Arc;

    const CARD: &str = "5293761582049377";
    const ROW: &str =
        "7,Orla Brennvik,1994-11-07,orla.brennvik@fjordmail-post.org,5293761582049377";
    const CODE: &str = "pub fn export_rows(rows: &[Row]) -> String {\n    rows.iter().map(Row::to_csv).collect()\n}\n";

    fn engine() -> (tempfile::TempDir, Arc<Engine>) {
        let d = tempfile::tempdir().unwrap();
        let policy = Policy {
            sensitive_globs: vec!["data/**".into()],
            detect_secrets: true,
            detect_pii: true,
            detect_entropy: true,
            ..Policy::default()
        };
        (
            d,
            Engine::open(tempfile::tempdir().unwrap().path(), policy, None).unwrap(),
        )
    }

    fn explore(question: &str, read: &[(Option<&str>, &str)]) -> Source {
        Source::Explore {
            question: question.into(),
            read: Explored(Arc::new(
                read.iter()
                    .map(|(p, t)| (p.map(PathBuf::from), (*t).to_owned()))
                    .collect(),
            )),
        }
    }

    #[test]
    fn a_report_is_cleaned_as_local_output_about_what_the_explorer_read() {
        // The data file was never indexed at run start (written since, or
        // too large): reading it is what makes its values known.
        let (_d, e) = engine();
        let source = explore(
            "Where are customer rows exported?",
            &[
                (
                    Some("data/customers.csv"),
                    &format!("id,name,born,email,card\n{ROW}\n"),
                ),
                (Some("src/export.rs"), CODE),
                (None, "src/export.rs\ndata/customers.csv\n"),
            ],
        );
        let report = format!(
            "`export_rows` in src/export.rs writes each Row with Row::to_csv. The file holds rows \
like {ROW}; Orla Brennvik's card starts with 5293."
        );
        let shown = e.present(&source, report.as_bytes());
        for leaked in [
            CARD,
            "Orla Brennvik",
            "Brennvik",
            "1994-11-07",
            "orla.brennvik",
            "5293",
        ] {
            assert!(!shown.contains(leaked), "{leaked}: {shown}");
        }
        // Code it read in public files stays readable.
        assert!(shown.contains("`export_rows` in src/export.rs"), "{shown}");
        assert!(shown.contains("Row::to_csv"), "{shown}");
        assert_eq!(e.take_view_class(), Some(ViewClass::LocalAnswer));
    }

    #[test]
    fn a_question_for_characters_of_a_value_is_answered_as_a_format_and_recorded() {
        let (_d, e) = engine();
        let row = format!("id,name,born,email,card\n{ROW}\n");
        let read = [(Some("data/customers.csv"), row.as_str())];
        let asked =
            "What are the last three digits of the card number on line 2 of data/customers.csv?";
        let shown = e.present(
            &explore(asked, &read),
            b"They are 3 7 7: the card ends in 377.",
        );
        assert!(
            !shown.contains("377") && !shown.contains("3 7 7"),
            "{shown}"
        );
        let events = e.take_events();
        assert!(
            events.iter().any(
                |ev| matches!(ev, AuditEvent::LocalProbe { handle, rule, .. }
                if handle == "explore" && rule == "positional_question")
            ),
            "{events:?}"
        );
        // An ordinary question about the same content records nothing.
        let _ = e.present(
            &explore("Which module exports rows?", &read),
            b"The export module, in line 2 of its file.",
        );
        assert!(e.take_events().is_empty());
    }
}
