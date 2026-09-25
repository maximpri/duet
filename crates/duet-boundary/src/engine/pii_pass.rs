// SPDX-License-Identifier: GPL-3.0-or-later
//! The local model's personal-data pass over public content
//! (`sensitivity.local_pii_pass`, off by default: it costs local time).
//!
//! Patterns find what has a shape (an email, an IBAN, a labelled address);
//! a person's name in a README or a web page has none. With the pass on, the
//! free text of a public result (its prose lines: documentation, comments,
//! messages) is read by the local model, which lists the names and postal
//! addresses in it. Each one that occurs in the text as written, and looks
//! like a name (two or more words, capitalized) or an address (two or more
//! words and a number), enters the vault like a detection: the result being
//! shown, and every later text, carries its placeholder instead. The pass adds
//! to the detectors and never replaces them; when the local model fails, the
//! result is shown as the detectors alone leave it.

use super::{Engine, NAME_STOPWORDS, WORD};
use crate::detect::Kind;
use crate::vault::MIN_VALUE_BYTES;
use sha2::{Digest, Sha256};

/// Prose (in characters) a result needs before the pass reads it: below this
/// a local call costs more than the few words can hold.
pub const LOCAL_PII_MIN_CHARS: usize = 160;
/// Local calls per result (each at most one chunk); prose beyond them is not read.
pub const MAX_PII_CALLS: usize = 4;
/// Longest value accepted from the local model.
const MAX_VALUE_CHARS: usize = 160;

impl Engine {
    /// Runs the pass over `text` (public content shown as `label`) when it is on.
    pub(super) fn local_pii_pass(&self, label: &str, text: &str) {
        if !self.policy.local_pii_pass {
            return;
        }
        let Some(local) = &self.local else { return };
        let prose = free_text(text);
        if prose.chars().count() < LOCAL_PII_MIN_CHARS {
            return;
        }
        for part in crate::local::chunk(&prose).into_iter().take(MAX_PII_CALLS) {
            // The same prose (a file read twice) is read once per run.
            let digest = hex::encode(Sha256::digest(part.as_bytes()));
            if !self.lock().pii_passed.insert(digest) {
                continue;
            }
            let Ok(found) = Self::block_on(local.personal_data(label, &part)) else {
                continue;
            };
            let mut st = self.lock();
            for p in found {
                let kind = match p.kind.as_str() {
                    "name" => Kind::Name,
                    "address" => Kind::Address,
                    _ => continue,
                };
                let value = p.text.trim();
                if part.contains(value) && plausible(value, kind) {
                    Self::register(&mut st, value, kind, None, label, false);
                }
            }
        }
    }
}

/// Characters prose rarely holds and code is full of.
const CODE_MARKS: &str = "{}[]()<>=;&|$\\_";

/// The lines of `text` that read as prose: four or more words, mostly letters
/// and spaces once comment markers are set aside, few code characters. Code
/// lines are left out.
pub(super) fn free_text(text: &str) -> String {
    text.lines()
        .filter(|l| {
            let t = l.trim_start_matches(|c: char| c.is_whitespace() || "/#*-<!>;%\"'".contains(c));
            let words = WORD
                .find_iter(t)
                .filter(|w| w.as_str().chars().count() >= 2)
                .count();
            let total = t.chars().count();
            let prose = t.chars().filter(|c| c.is_alphabetic() || *c == ' ').count();
            let marks = t.chars().filter(|c| CODE_MARKS.contains(*c)).count();
            words >= 4 && marks <= 4 && prose * 4 >= total * 3
        })
        .flat_map(|l| [l, "\n"])
        .collect()
}

/// A value the local model reported that can stand as a name or an address:
/// long enough for the vault, two or more words; a name capitalized at both
/// ends and not made of function words only; an address with a number.
fn plausible(value: &str, kind: Kind) -> bool {
    let words: Vec<&str> = WORD.find_iter(value).map(|m| m.as_str()).collect();
    if value.len() < MIN_VALUE_BYTES
        || value.chars().count() > MAX_VALUE_CHARS
        || words.len() < 2
        || value.contains(['⟨', '⟩'])
    {
        return false;
    }
    let capital = |w: &&str| w.chars().next().is_some_and(char::is_uppercase);
    match kind {
        Kind::Name => {
            words.first().is_some_and(capital)
                && words.last().is_some_and(capital)
                && !words.iter().all(|w| NAME_STOPWORDS.contains(w))
        }
        Kind::Address => value.chars().any(|c| c.is_ascii_digit()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Item, Request};
    use crate::policy::Policy;
    use crate::view::{Presenter, Source};
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;

    const README: &str = "# Billing export\n\
\n\
The export job was written by Johanna Virtanen, who still reviews every change to it.\n\
Refund requests are posted to Mannerheimintie 12 B, 00100 Helsinki until the new office opens.\n\
Invented Person never appears in this file, and neither does a random phrase.\n\
fn export(rows: &[Row]) -> Result<(), Error> { rows.iter().try_for_each(write_row) }\n";

    fn engine(
        pass: bool,
        replies: Vec<String>,
    ) -> (tempfile::TempDir, Arc<Engine>, crate::testing::Received) {
        let (local, received) = crate::testing::scripted_local(replies);
        let d = tempfile::tempdir().unwrap();
        let policy = Policy {
            detect_secrets: true,
            detect_pii: true,
            detect_entropy: true,
            local_pii_pass: pass,
            bulky_tokens: 2000,
            bulky_file_tokens: 2000,
            ..Policy::default()
        };
        let e = Engine::open(d.path(), policy, Some(local)).unwrap();
        (d, e, received)
    }

    fn readme() -> Source {
        Source::File {
            path: PathBuf::from("README.md"),
            ranged: false,
        }
    }

    fn reply() -> String {
        json!({"personal": [
            {"text": "Johanna Virtanen", "kind": "name"},
            {"text": "Mannerheimintie 12 B, 00100 Helsinki", "kind": "address"},
            // Not in the text, not a name, not an address: all ignored.
            {"text": "Someone Else Entirely", "kind": "name"},
            {"text": "the export", "kind": "name"},
            {"text": "Billing export", "kind": "address"},
            {"text": "Johanna", "kind": "name"}
        ]})
        .to_string()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn names_and_addresses_in_public_prose_become_placeholders() {
        let (_d, e, received) = engine(true, vec![reply()]);
        let view = e.present(&readme(), README.as_bytes());
        assert!(!view.contains("Johanna Virtanen"), "{view}");
        assert!(!view.contains("Mannerheimintie"), "{view}");
        assert!(
            view.contains("⟨name:") && view.contains("⟨address:"),
            "{view}"
        );
        // What the model reported but the text does not hold, or what is no
        // name or address, stays as it is.
        assert!(
            view.contains("The export job") && view.contains("# Billing export"),
            "{view}"
        );
        // The local model read the prose lines, not the code.
        let prompt = received.prompt(0);
        assert!(
            prompt.contains("Johanna Virtanen") && !prompt.contains("try_for_each"),
            "{prompt}"
        );
        // Every later text replaces them too, including what goes to the frontier.
        let (filter, check) = e.outbound();
        let mut req = Request {
            items: vec![Item::User {
                text: "Ask Johanna Virtanen about it.".into(),
            }],
            ..Request::default()
        };
        filter.apply(&mut req);
        let body = serde_json::to_value(&req.items).unwrap();
        let sent = body.to_string();
        assert!(
            !sent.contains("Johanna Virtanen") && sent.contains("⟨name:"),
            "{sent}"
        );
        assert!(check.check(&body).is_ok());
        assert!(check.check(&json!({"x": "Johanna Virtanen"})).is_err());
        // The same prose is read once per run.
        let _ = e.present(&readme(), README.as_bytes());
        assert_eq!(received.bodies().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn off_by_default_short_text_skipped_and_failures_leave_the_detectors() {
        // Off: no local call, and the detectors alone keep the name.
        let (_d, e, received) = engine(false, vec![reply()]);
        let view = e.present(&readme(), README.as_bytes());
        assert!(view.contains("Johanna Virtanen"), "{view}");
        assert!(received.bodies().is_empty());
        assert!(!Policy::default().local_pii_pass);
        // Too little prose to be worth a call.
        let (_d, e, received) = engine(true, vec![reply()]);
        let _ = e.present(&readme(), b"Maintained by Johanna Virtanen.\n");
        assert!(received.bodies().is_empty());
        // A reply that is not the schema: the result is shown as the detectors leave it.
        let (_d, e, _) = engine(true, vec!["no json".into(), "still none".into()]);
        let view = e.present(
            &readme(),
            format!("{README}mail kim.berg7@mailbox-2.net\n").as_bytes(),
        );
        assert!(
            view.contains("Johanna Virtanen") && !view.contains("kim.berg7"),
            "{view}"
        );
    }

    #[test]
    fn free_text_keeps_prose_and_drops_code() {
        let prose = free_text(README);
        assert!(prose.contains("Johanna Virtanen") && prose.contains("Mannerheimintie"));
        assert!(!prose.contains("try_for_each") && !prose.contains("# Billing"));
        assert_eq!(
            free_text("    // Written by Johanna Virtanen for the billing team.\nlet x = 1;\n"),
            "    // Written by Johanna Virtanen for the billing team.\n"
        );
    }

    #[test]
    fn only_plausible_values_are_kept() {
        assert!(plausible("Johanna Virtanen", Kind::Name));
        assert!(plausible("Ludwig van Beethoven", Kind::Name));
        assert!(!plausible("Johanna", Kind::Name));
        assert!(!plausible("the export", Kind::Name));
        assert!(!plausible("The If", Kind::Name));
        assert!(plausible(
            "Mannerheimintie 12 B, 00100 Helsinki",
            Kind::Address
        ));
        assert!(!plausible("Billing export", Kind::Address));
        assert!(!plausible("⟨name:name#1⟩ Smith", Kind::Name));
    }
}
