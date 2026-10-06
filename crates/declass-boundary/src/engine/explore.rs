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
//! and every line quoting protected code; before that, with structure views
//! on, every value of structured sensitive data it names is withheld (a date
//! of birth in a customer file, which no detector knows, written outside a
//! copied run).
//! What it read joins the indexes first: sensitive text as sensitive (a file
//! the run-start index did not take, or one written since), protected source
//! through its own index, and public files as public (code they share is
//! never taken for a quotation, and their words are never taken for names).

use super::structure::Known;
use super::{Answered, Engine, PLACEHOLDER, State, TERM, words};
use crate::audit::AuditEvent;
use crate::detect::Kind;
use crate::probing;
use crate::structure::Knowledge;
use crate::view::{Explored, ViewClass};

/// The handle name the explorer's probes are counted under.
const PROBES: &str = "explore";

/// Shown in place of a value of structured sensitive data in a report.
pub const DATA_WITHHELD: &str = "⟨withheld:a-value-of-the-data⟩";

/// `text` with every value of structured sensitive data it names withheld
/// (the engine knows the string values of the sensitive data files it
/// indexed), except one made only of words public content or the schema
/// uses (`pending`, when the code names that status too): a value with a
/// digit is always withheld. Returns the text and how many.
fn withhold_data_values(st: &State, text: &str) -> (String, usize) {
    let known = Known(st);
    let bracketed: Vec<(usize, usize)> = PLACEHOLDER
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect();
    let mut out = String::with_capacity(text.len());
    let (mut last, mut n) = (0, 0);
    for (s, e, kind) in known.values(text) {
        if kind != Kind::Data || s < last || bracketed.iter().any(|&(bs, be)| s < be && bs < e) {
            continue;
        }
        let value = &text[s..e];
        let public = !value.chars().any(|c| c.is_ascii_digit())
            && TERM
                .find_iter(value)
                .all(|w| known.public_word(&w.as_str().to_lowercase()));
        if public {
            continue;
        }
        out.push_str(&text[last..s]);
        out.push_str(DATA_WITHHELD);
        last = e;
        n += 1;
    }
    out.push_str(&text[last..]);
    (out, n)
}

impl Engine {
    pub(super) fn explore_view(&self, question: &str, read: &Explored, text: &str) -> String {
        self.set_class(ViewClass::LocalAnswer);
        let mut st = self.lock();
        for (path, t) in read.0.iter() {
            let Some(path) = path else { continue };
            let digest = declass_fs::sha256_hex(format!("{}\0{t}", path.display()).as_bytes());
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
        // Values of the data first, on the report as written: once pieces of
        // a value are withheld, the rest no longer matches it.
        let (text, _) = withhold_data_values(&st, text);
        let (clean, withheld) =
            self.clean_local_counted(&mut st, &text, "explorer report", &all, Some(answered));
        let (clean, _) = Self::ip_redact(&mut st, &clean);
        if narrow || withheld > 0 {
            let count = st.probes.probe(PROBES);
            let _ = declass_fs::private::write_private(
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
    fn values_of_structured_data_are_withheld_wherever_the_report_names_them() {
        // No detector knows a date of birth or a plan name; with structure
        // views on, the engine knows them as values of the customer file.
        let d = tempfile::tempdir().unwrap();
        let policy = Policy {
            sensitive_globs: vec!["data/**".into()],
            detect_pii: true,
            structure: crate::policy::StructureSettings {
                views: true,
                ..Default::default()
            },
            ..Policy::default()
        };
        let e = Engine::open(d.path(), policy, None).unwrap();
        let ws = d.path().join("ws");
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(
            ws.join("data/customers.csv"),
            "id,born,plan,status\n7,1994-11-07,Aurora Gold,pending\n",
        )
        .unwrap();
        std::fs::write(ws.join("src/lib.rs"), "// status: pending or paid\n").unwrap();
        e.prime(&ws, &["data/customers.csv".into(), "src/lib.rs".into()], "");
        let shown = e.present(
            &explore(
                "Which customers are pending?",
                &[(Some("src/lib.rs"), "// status: pending or paid\n")],
            ),
            b"Customer 7 (born 1994-11-07, plan Aurora Gold) is still pending.",
        );
        assert!(
            !shown.contains("1994-11-07") && !shown.contains("Aurora Gold"),
            "{shown}"
        );
        assert!(shown.contains(super::DATA_WITHHELD), "{shown}");
        // A status the public code also names is structure, not a secret.
        assert!(shown.contains("still pending"), "{shown}");
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
