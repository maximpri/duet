// SPDX-License-Identifier: GPL-3.0-or-later
//! Imported secret rules, maintained elsewhere and used as data: the gitleaks
//! default rule set (`rules/gitleaks.toml`, MIT; version and hash pinned in
//! `rules/NOTICE`), compiled into declass's own detector on first use.
//!
//! Of each rule declass uses its expression, keywords, entropy threshold, secret
//! group, path condition and allowlists, and the file's global allowlist:
//! - Expressions are RE2 syntax. They are compiled by the Rust engine over
//!   bytes with Unicode classes off, which reads `\w`, `\d`, `\s`, `\b` and
//!   case folding as ASCII, as RE2 does; the few escapes the two engines read
//!   differently are rewritten first ([`translate`]). An expression neither
//!   mode accepts is kept with its error in [`RuleSet::failed`], never dropped
//!   silently: the corpus test lists those rules and fails when they grow.
//! - Keywords are a prefilter: one ASCII case-insensitive Aho-Corasick pass
//!   over the text finds the rules that can match at all, and only those run.
//! - The secret is the rule's secret group, else its first non-empty group,
//!   else the whole match. A secret whose entropy is at or below the rule's
//!   threshold, or one an allowlist allows, is not a finding.
//! - What is withheld is that secret, except where the first non-empty group
//!   is a marker rather than the value: a group inside a repetition
//!   (`([a-z0-9]{4}-){3}` captures one piece of an id) or one of several
//!   alternative groups (`(?P<alg>…)|(?P<typ>…)` names a JWT header field).
//!   Then the whole match is withheld, never a fragment of the secret.
//! - A rule with a path condition runs only on text from a matching path. A
//!   rule that only names files (no expression) has nothing to find in text
//!   and is listed in [`RuleSet::path_only`].
//!
//! Declass's own detectors ([`crate::detect`]) stay and take precedence: an
//! imported match inside a span they found is dropped.

use aho_corasick::{AhoCorasick, MatchKind};
use regex::bytes::{Regex, RegexBuilder};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// The vendored rule file, unmodified.
pub const GITLEAKS_TOML: &str = include_str!("../rules/gitleaks.toml");
/// Where the rule file came from: version, commit, sha256, license.
pub const NOTICE: &str = include_str!("../rules/NOTICE");

/// Compiled size one expression may take. Counted repetitions over classes
/// (`[\w.-]{0,50}?`) unroll; the engine's default of 10 MB is too tight for a few.
const SIZE_LIMIT: usize = 64 << 20;

static IMPORTED: LazyLock<RuleSet> = LazyLock::new(|| RuleSet::compile(GITLEAKS_TOML));

/// The imported rule set, compiled on first use.
pub fn imported() -> &'static RuleSet {
    &IMPORTED
}

/// A field of the pinned `NOTICE` (`version`, `sha256`, ...).
pub fn notice_field(name: &str) -> Option<&'static str> {
    NOTICE.lines().find_map(|l| {
        l.strip_prefix(name)
            .and_then(|r| r.strip_prefix(':'))
            .map(str::trim)
    })
}

#[derive(Deserialize)]
struct RawFile {
    /// The older single global allowlist.
    allowlist: Option<RawAllowlist>,
    #[serde(default)]
    allowlists: Vec<RawAllowlist>,
    #[serde(default)]
    rules: Vec<RawRule>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRule {
    id: String,
    #[serde(default)]
    description: String,
    regex: Option<String>,
    #[serde(default)]
    secret_group: usize,
    entropy: Option<f64>,
    path: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    allowlist: Option<RawAllowlist>,
    #[serde(default)]
    allowlists: Vec<RawAllowlist>,
    /// Fields this reader does not implement (composite rules, report
    /// switches): the rule is compiled without them and listed as partial.
    #[serde(flatten)]
    other: BTreeMap<String, toml::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAllowlist {
    condition: Option<String>,
    regex_target: Option<String>,
    #[serde(default)]
    regexes: Vec<String>,
    #[serde(default)]
    stopwords: Vec<String>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    commits: Vec<String>,
}

/// Rule fields that carry no detection semantics.
const DESCRIPTIVE_FIELDS: &[&str] = &["tags"];

/// A rule that did not compile, or compiled without part of its definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub rule: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Secret,
    Match,
    Line,
}

/// What an allowlist is checked against.
struct Candidate<'a> {
    secret: &'a [u8],
    matched: &'a [u8],
    line: &'a [u8],
    path: Option<&'a str>,
}

struct Allowlist {
    /// `condition = "AND"`: every criterion given must hold (else any one).
    all: bool,
    target: Target,
    regexes: Vec<Regex>,
    /// Words that make a secret an allowed one when it contains them.
    stopwords: Option<AhoCorasick>,
    paths: Vec<Regex>,
    /// Commit criteria never hold: declass scans text, not commits.
    commits: bool,
}

impl Allowlist {
    fn compile(raw: &RawAllowlist, owner: &str, partial: &mut Vec<Failure>) -> Self {
        let mut regexes = |list: &[String], what: &str| -> Vec<Regex> {
            list.iter()
                .filter_map(|p| match compile(p) {
                    Ok(r) => Some(r),
                    Err(e) => {
                        partial.push(Failure {
                            rule: owner.to_owned(),
                            reason: format!("allowlist {what} {p:?} not compiled: {e}"),
                        });
                        None
                    }
                })
                .collect()
        };
        let target = match raw.regex_target.as_deref() {
            Some("match") => Target::Match,
            Some("line") => Target::Line,
            _ => Target::Secret,
        };
        let stopwords = (!raw.stopwords.is_empty())
            .then(|| {
                AhoCorasick::builder()
                    .ascii_case_insensitive(true)
                    .build(&raw.stopwords)
                    .ok()
            })
            .flatten();
        Self {
            all: raw
                .condition
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case("and")),
            target,
            regexes: regexes(&raw.regexes, "regex"),
            stopwords,
            paths: regexes(&raw.paths, "path"),
            commits: !raw.commits.is_empty(),
        }
    }

    fn allows(&self, c: &Candidate<'_>) -> bool {
        let target = match self.target {
            Target::Secret => c.secret,
            Target::Match => c.matched,
            Target::Line => c.line,
        };
        let checks = [
            (!self.regexes.is_empty()).then(|| self.regexes.iter().any(|r| r.is_match(target))),
            self.stopwords.as_ref().map(|s| s.is_match(c.secret)),
            (!self.paths.is_empty()).then(|| self.path_matches(c.path)),
            self.commits.then_some(false),
        ];
        let mut given = checks.into_iter().flatten().peekable();
        if self.all {
            given.peek().is_some() && given.all(|ok| ok)
        } else {
            given.any(|ok| ok)
        }
    }

    fn path_matches(&self, path: Option<&str>) -> bool {
        path.is_some_and(|p| self.paths.iter().any(|r| r.is_match(p.as_bytes())))
    }

    /// Allows everything found in text from `path`, whatever it is.
    fn allows_whole(&self, path: Option<&str>) -> bool {
        !self.all && self.path_matches(path)
    }
}

pub struct Rule {
    id: String,
    label: String,
    description: String,
    pattern: String,
    regex: Regex,
    secret_group: usize,
    /// Capture groups that mark a part or a variant rather than hold the secret.
    markers: Vec<usize>,
    entropy: Option<f64>,
    path_pattern: Option<String>,
    path: Option<Regex>,
    keywords: Vec<String>,
    allowlists: Vec<Allowlist>,
}

impl Rule {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The id as a placeholder label (`aws-access-token` → `aws_access_token`).
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    /// The expression as written in the rule file.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    pub fn path_pattern(&self) -> Option<&str> {
        self.path_pattern.as_deref()
    }

    pub fn keywords(&self) -> &[String] {
        &self.keywords
    }

    pub fn entropy(&self) -> Option<f64> {
        self.entropy
    }
}

/// A secret an imported rule found: byte span (on character boundaries).
#[derive(Clone, Copy)]
pub struct RuleMatch<'a> {
    pub rule: &'a Rule,
    pub start: usize,
    pub end: usize,
}

impl std::fmt::Debug for RuleMatch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}..{}", self.rule.id, self.start, self.end)
    }
}

pub struct RuleSet {
    rules: Vec<Rule>,
    /// One automaton over every distinct keyword; pattern `i` enables `keyword_rules[i]`.
    keywords: Option<AhoCorasick>,
    keyword_rules: Vec<Vec<usize>>,
    /// Rules without keywords run on every text.
    unfiltered: Vec<usize>,
    global: Vec<Allowlist>,
    failed: Vec<Failure>,
    partial: Vec<Failure>,
    path_only: Vec<String>,
}

impl RuleSet {
    /// Compiles a gitleaks-format rule file. Never fails: what cannot be used
    /// is recorded in [`Self::failed`] and [`Self::partial`].
    pub fn compile(toml_text: &str) -> Self {
        let mut set = Self {
            rules: Vec::new(),
            keywords: None,
            keyword_rules: Vec::new(),
            unfiltered: Vec::new(),
            global: Vec::new(),
            failed: Vec::new(),
            partial: Vec::new(),
            path_only: Vec::new(),
        };
        let raw: RawFile = match toml::from_str(toml_text) {
            Ok(r) => r,
            Err(e) => {
                set.failed.push(Failure {
                    rule: "(rule file)".into(),
                    reason: format!("not a rule file: {e}"),
                });
                return set;
            }
        };
        for a in raw.allowlist.iter().chain(&raw.allowlists) {
            let a = Allowlist::compile(a, "(global allowlist)", &mut set.partial);
            set.global.push(a);
        }
        let mut by_keyword: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for r in raw.rules {
            for key in r.other.keys() {
                if !DESCRIPTIVE_FIELDS.contains(&key.as_str()) {
                    set.partial.push(Failure {
                        rule: r.id.clone(),
                        reason: format!("field `{key}` is not supported; the rule runs without it"),
                    });
                }
            }
            let Some(pattern) = r.regex.clone() else {
                set.path_only.push(r.id);
                continue;
            };
            let regex = match compile(&pattern) {
                Ok(re) => re,
                Err(e) => {
                    set.failed.push(Failure {
                        rule: r.id,
                        reason: e,
                    });
                    continue;
                }
            };
            let path = match r.path.as_deref().map(compile).transpose() {
                Ok(p) => p,
                Err(e) => {
                    set.failed.push(Failure {
                        rule: r.id,
                        reason: format!("path condition: {e}"),
                    });
                    continue;
                }
            };
            let allowlists = r
                .allowlist
                .iter()
                .chain(&r.allowlists)
                .map(|a| Allowlist::compile(a, &r.id, &mut set.partial))
                .collect();
            let index = set.rules.len();
            if r.keywords.is_empty() {
                set.unfiltered.push(index);
            }
            for k in &r.keywords {
                by_keyword
                    .entry(k.to_ascii_lowercase())
                    .or_default()
                    .push(index);
            }
            set.rules.push(Rule {
                label: r.id.replace('-', "_"),
                markers: marker_groups(&pattern),
                id: r.id,
                description: r.description,
                pattern,
                regex,
                secret_group: r.secret_group,
                entropy: r.entropy.filter(|e| *e > 0.0),
                path_pattern: r.path,
                path,
                keywords: r.keywords,
                allowlists,
            });
        }
        let (words, rules): (Vec<String>, Vec<Vec<usize>>) = by_keyword.into_iter().unzip();
        set.keywords = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .match_kind(MatchKind::Standard)
            .build(&words)
            .ok();
        set.keyword_rules = rules;
        set
    }

    /// Rules compiled and in use.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Rules whose expression or path condition did not compile (not in use).
    pub fn failed(&self) -> &[Failure] {
        &self.failed
    }

    /// Rules in use without part of their definition (an allowlist entry that
    /// did not compile, an unsupported field).
    pub fn partial(&self) -> &[Failure] {
        &self.partial
    }

    /// Rules that only name files (no expression).
    pub fn path_only(&self) -> &[String] {
        &self.path_only
    }

    /// The rule whose placeholder label is `label`.
    pub fn by_label(&self, label: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.label == label)
    }

    /// Secrets the rules find in `text`, sorted by start (longer first).
    /// `path` is where the text came from, for path conditions and allowlists.
    pub fn find(&self, text: &str, path: Option<&str>) -> Vec<RuleMatch<'_>> {
        self.find_counted(text, path).0
    }

    /// [`Self::find`], and how many rules got past the keyword prefilter and ran.
    pub fn find_counted(&self, text: &str, path: Option<&str>) -> (Vec<RuleMatch<'_>>, usize) {
        if self.global.iter().any(|a| a.allows_whole(path)) {
            return (Vec::new(), 0);
        }
        let hay = text.as_bytes();
        let mut active = vec![false; self.rules.len()];
        for &i in &self.unfiltered {
            active[i] = true;
        }
        if let Some(ac) = &self.keywords {
            for m in ac.find_overlapping_iter(hay) {
                for &i in &self.keyword_rules[m.pattern().as_usize()] {
                    active[i] = true;
                }
            }
        }
        let mut out = Vec::new();
        let mut ran = 0;
        for (rule, _) in self.rules.iter().zip(&active).filter(|(_, on)| **on) {
            if let Some(p) = &rule.path
                && !path.is_some_and(|p2| p.is_match(p2.as_bytes()))
            {
                continue;
            }
            ran += 1;
            for caps in rule.regex.captures_iter(hay) {
                let Some(whole) = caps.get(0) else { continue };
                let first = |skip: &[usize]| {
                    (1..caps.len())
                        .filter(|g| !skip.contains(g))
                        .filter_map(|g| caps.get(g))
                        .find(|m| !m.is_empty())
                };
                let (secret, withheld) = if rule.secret_group > 0 {
                    let g = caps.get(rule.secret_group);
                    (g, g)
                } else {
                    (first(&[]), first(&rule.markers))
                };
                let (secret, withheld) = (secret.unwrap_or(whole), withheld.unwrap_or(whole));
                if secret.is_empty()
                    || rule
                        .entropy
                        .is_some_and(|min| shannon(secret.as_bytes()) <= min)
                {
                    continue;
                }
                let candidate = Candidate {
                    secret: secret.as_bytes(),
                    matched: whole.as_bytes(),
                    line: line_of(hay, whole.start(), whole.end()),
                    path,
                };
                if rule
                    .allowlists
                    .iter()
                    .chain(&self.global)
                    .any(|a| a.allows(&candidate))
                {
                    continue;
                }
                let (start, end) = on_char_boundaries(text, withheld.start(), withheld.end());
                out.push(RuleMatch { rule, start, end });
            }
        }
        out.sort_by_key(|m| (m.start, std::cmp::Reverse(m.end)));
        out.dedup_by(|a, b| a.start == b.start && a.end == b.end && a.rule.id == b.rule.id);
        (out, ran)
    }
}

/// The capture groups of `pattern` that mark a part or a variant: groups
/// inside a repetition, and groups that are alternatives of one another.
fn marker_groups(pattern: &str) -> Vec<usize> {
    use regex_syntax::hir::{Hir, HirKind};
    fn walk(h: &Hir, repeated: bool, out: &mut Vec<usize>) {
        match h.kind() {
            HirKind::Capture(c) => {
                if repeated {
                    out.push(c.index as usize);
                }
                walk(&c.sub, repeated, out);
            }
            HirKind::Repetition(r) => walk(&r.sub, repeated || r.max != Some(1), out),
            HirKind::Concat(parts) => parts.iter().for_each(|p| walk(p, repeated, out)),
            HirKind::Alternation(alts) => {
                let groups = alts
                    .iter()
                    .filter(|a| matches!(a.kind(), HirKind::Capture(_)))
                    .count();
                for a in alts {
                    if let HirKind::Capture(c) = a.kind()
                        && groups >= 2
                    {
                        out.push(c.index as usize);
                    }
                    walk(a, repeated, out);
                }
            }
            _ => {}
        }
    }
    let translated = translate(pattern);
    let hir = [false, true].into_iter().find_map(|unicode| {
        regex_syntax::ParserBuilder::new()
            .unicode(unicode)
            .utf8(false)
            .build()
            .parse(&translated)
            .ok()
    });
    let mut out = Vec::new();
    if let Some(h) = hir {
        walk(&h, false, &mut out);
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Compiles an RE2 expression with RE2's ASCII reading of classes; one only
/// the Unicode mode accepts (a `\p{..}` class) is compiled in that mode.
fn compile(pattern: &str) -> Result<Regex, String> {
    let translated = translate(pattern);
    let build = |unicode: bool| {
        RegexBuilder::new(&translated)
            .unicode(unicode)
            .size_limit(SIZE_LIMIT)
            .build()
    };
    build(false)
        .or_else(|_| build(true))
        .map_err(|e| e.to_string().lines().last().unwrap_or_default().to_owned())
}

/// Rewrites what the Rust engine reads differently from RE2: `\\<` and `\\>`
/// are literal characters in RE2 but word boundaries in Rust; `\\Q…\\E` quotes
/// literal text in RE2 only; a `{` that starts no repetition (`${VAR}`) is a
/// literal in RE2 and an error in Rust; `\\_` is an escaped underscore.
pub fn translate(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut rest = pattern;
    while let Some(c) = rest.chars().next() {
        let after = &rest[c.len_utf8()..];
        match c {
            '\\' => match after.chars().next() {
                Some(e @ ('<' | '>' | '_')) => {
                    out.push(e);
                    rest = &after[1..];
                }
                Some('Q') => {
                    let body = &after[1..];
                    let (quoted, tail) = body.split_once("\\E").unwrap_or((body, ""));
                    out.push_str(&regex::escape(quoted));
                    rest = tail;
                }
                // `\p{Greek}`, `\x{263a}`: the braces belong to the escape.
                Some('p' | 'P' | 'x') if after[1..].starts_with('{') => {
                    let close = after.find('}').map_or(after.len(), |i| i + 1);
                    out.push('\\');
                    out.push_str(&after[..close]);
                    rest = &after[close..];
                }
                Some(e) => {
                    out.push('\\');
                    out.push(e);
                    rest = &after[e.len_utf8()..];
                }
                None => {
                    out.push_str("\\\\");
                    rest = after;
                }
            },
            '{' if !starts_repetition(after) => {
                out.push_str("\\{");
                rest = after;
            }
            _ => {
                out.push(c);
                rest = after;
            }
        }
    }
    out
}

/// Whether `s` (the text after a `{`) completes a repetition: `n}`, `n,}` or `n,m}`.
fn starts_repetition(s: &str) -> bool {
    let digits = |t: &str| t.len() - t.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let n = digits(s);
    if n == 0 {
        return false;
    }
    let t = &s[n..];
    match t.strip_prefix(',') {
        Some(u) => u[digits(u)..].starts_with('}'),
        None => t.starts_with('}'),
    }
}

/// Shannon entropy in bits per byte.
fn shannon(bytes: &[u8]) -> f64 {
    let mut counts = [0u32; 256];
    for &b in bytes {
        counts[b as usize] += 1;
    }
    let n = bytes.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = f64::from(c) / n;
            -p * p.log2()
        })
        .sum()
}

/// The lines holding `start..end`, without their line breaks.
fn line_of(hay: &[u8], start: usize, end: usize) -> &[u8] {
    let from = hay[..start]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let to = hay[end..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(hay.len(), |i| end + i);
    &hay[from..to.max(from)]
}

/// `start..end` widened to character boundaries (a byte-level match may end
/// inside a multi-byte character; the whole character is withheld).
fn on_char_boundaries(text: &str, mut start: usize, mut end: usize) -> (usize, usize) {
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    while !text.is_char_boundary(end) {
        end += 1;
    }
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// A small rule file in the vendored format, exercising each feature.
    const RULES: &str = r#"
[allowlist]
description = "global"
regexes = ['''^EXAMPLE''']
stopwords = ["sample"]
paths = ['''(?:^|/)vendor/''']

[[rules]]
id = "demo-key"
regex = '''\b(demo_[a-z0-9]{12})\b'''
entropy = 3.0
keywords = ["demo_"]

[[rules]]
id = "labelled-token"
regex = '''(?i)(?:widget)[ \t]*[=:][ \t]*["']?([a-z0-9]{16})'''
keywords = ["widget"]
[[rules.allowlists]]
regexTarget = "line"
regexes = ['''#\s*fixture''']
[[rules.allowlists]]
regexTarget = "match"
regexes = ['''(?i)widget_id''']

[[rules]]
id = "grouped"
regex = '''(grp)-([A-Z]{4})-([0-9]{6})'''
secretGroup = 3
keywords = ["grp-"]

[[rules]]
id = "yaml-only"
regex = '''pass:\s*(\S{8,})'''
path = '''\.ya?ml$'''
keywords = ["pass:"]
[[rules.allowlists]]
condition = "AND"
paths = ['''test''']
regexes = ['''^dummy''']

[[rules]]
id = "backreference"
regex = '''(a)\1'''
keywords = ["a"]

[[rules]]
id = "composite"
regex = '''cmp_[0-9]{8}'''
keywords = ["cmp_"]
[[rules.required]]
id = "demo-key"

[[rules]]
id = "names-files-only"
path = '''\.p12$'''
"#;

    fn ids(set: &RuleSet, text: &str, path: Option<&str>) -> Vec<(String, String)> {
        set.find(text, path)
            .iter()
            .map(|m| (m.rule.id().to_owned(), text[m.start..m.end].to_owned()))
            .collect()
    }

    fn one(id: &str, value: &str) -> Vec<(String, String)> {
        vec![(id.to_owned(), value.to_owned())]
    }

    #[test]
    fn rule_features_as_the_file_defines_them() {
        let set = RuleSet::compile(RULES);
        assert_eq!(set.rules().len(), 5);
        // Expressions the engine rejects, and path-only rules, are listed.
        assert_eq!(set.failed().len(), 1);
        assert_eq!(set.failed()[0].rule, "backreference");
        assert_eq!(set.path_only(), ["names-files-only"]);
        assert_eq!(set.partial().len(), 1);
        assert!(set.partial()[0].reason.contains("required"));

        // Entropy: the secret must exceed the threshold.
        assert_eq!(
            ids(&set, "k demo_a1b2c3d4e5f6 ", None),
            one("demo-key", "demo_a1b2c3d4e5f6")
        );
        assert!(ids(&set, "k demo_aaaaaaaaaaaa ", None).is_empty());
        // The first non-empty group is the secret; allowlists by line and match.
        assert_eq!(
            ids(&set, "widget = 'q7w8e9r0t1y2u3i4'", None),
            one("labelled-token", "q7w8e9r0t1y2u3i4")
        );
        assert!(ids(&set, "widget = 'q7w8e9r0t1y2u3i4' # fixture", None).is_empty());
        assert!(ids(&set, "widget_id: q7w8e9r0t1y2u3i4", None).is_empty());
        // secretGroup names the group.
        assert_eq!(ids(&set, "grp-ABCD-123456", None), one("grouped", "123456"));
        // A path condition: only in YAML files; an AND allowlist needs every criterion.
        let yaml = "pass: hunter2hunter2";
        assert!(ids(&set, yaml, None).is_empty());
        assert!(ids(&set, yaml, Some("cfg/app.toml")).is_empty());
        assert_eq!(
            ids(&set, yaml, Some("cfg/app.yaml")),
            one("yaml-only", "hunter2hunter2")
        );
        assert_eq!(
            ids(&set, yaml, Some("test/app.yaml")),
            one("yaml-only", "hunter2hunter2")
        );
        assert!(ids(&set, "pass: dummy_value_1", Some("test/app.yaml")).is_empty());
        // The global allowlist: secret regexes, stopwords, paths.
        assert!(ids(&set, "demo_samplex9y8z7", None).is_empty());
        assert!(ids(&set, "k demo_a1b2c3d4e5f6 ", Some("vendor/lib/x.go")).is_empty());
        // A rule whose unsupported field was skipped still runs.
        assert_eq!(
            ids(&set, "cmp_12345678", None),
            one("composite", "cmp_12345678")
        );
    }

    #[test]
    fn marker_groups_never_leave_the_rest_of_the_secret() {
        let set = RuleSet::compile(
            r#"
[[rules]]
id = "hook"
regex = '''https://h\.test/[a-z0-9]{4}-([a-z0-9]{2}-){2}[a-z0-9]{6}'''
keywords = ["h.test"]

[[rules]]
id = "variants"
regex = '''\btok(?:(?P<a>AAA)|(?P<b>BBB))[a-z0-9]{12}'''
keywords = ["tok"]

[[rules]]
id = "nested"
regex = '''key: ((live|test)_[a-f0-9]{8})'''
keywords = ["key:"]
"#,
        );
        assert_eq!(
            ids(&set, "see https://h.test/ab12-cd-ef-123456 now", None),
            one("hook", "https://h.test/ab12-cd-ef-123456")
        );
        assert_eq!(
            ids(&set, "x tokBBBq1w2e3r4t5y6 y", None),
            one("variants", "tokBBBq1w2e3r4t5y6")
        );
        // An outer group holding an inner one is the secret, as written.
        assert_eq!(
            ids(&set, "key: live_0a1b2c3d", None),
            one("nested", "live_0a1b2c3d")
        );
        assert_eq!(marker_groups(r"(a)|(b)"), [1, 2]);
        assert_eq!(marker_groups(r"x(a)?(b){2}"), [2]);
        assert!(marker_groups(r"((live|test)_x)").is_empty());
    }

    #[test]
    fn keywords_decide_which_rules_run() {
        let set = RuleSet::compile(RULES);
        let (found, ran) = set.find_counted("nothing to see here", None);
        assert!(found.is_empty() && ran == 0);
        // Keywords match case-insensitively; only the rules they name run.
        let (_, ran) = set.find_counted("WIDGET = 1", None);
        assert_eq!(ran, 1);
        // The vendored set: ordinary code runs few of its 200+ rules.
        let code = "fn parse(line: &str) -> Result<Header, Error> {\n    let n = line.len();\n}\n";
        let (found, ran) = imported().find_counted(code, None);
        assert!(found.is_empty());
        assert!(ran < 5, "{ran} rules ran on plain code");
    }

    #[test]
    fn expressions_read_as_re2_reads_them() {
        assert_eq!(translate(r"a\<b\>c"), "a<b>c");
        assert_eq!(translate(r"\Qa.b*\E+"), r"a\.b\*+");
        assert_eq!(translate(r"x\_y\\z\d"), r"x_y\\z\d");
        assert_eq!(translate(r"^\$(?:\d+|{\d+})$"), r"^\$(?:\d+|\{\d+})$");
        assert_eq!(translate(r"a{2}b{1,}c{1,3}"), r"a{2}b{1,}c{1,3}");
        assert_eq!(translate(r"\p{Greek}\x{263a}"), r"\p{Greek}\x{263a}");
        // `\w` and `\b` are ASCII, as in RE2.
        let re = compile(r"\b\w+\b").unwrap();
        let found: Vec<&[u8]> = re
            .find_iter("café ok".as_bytes())
            .map(|m| m.as_bytes())
            .collect();
        assert_eq!(found, [b"caf".as_slice(), b"ok".as_slice()]);
        // Unicode classes still compile.
        assert!(compile(r"\p{Greek}+").is_ok());
    }

    #[test]
    fn spans_end_on_character_boundaries() {
        let set = RuleSet::compile(
            r#"
[[rules]]
id = "any"
regex = '''key=(.{6})'''
keywords = ["key="]
"#,
        );
        let text = "key=abcdeé!";
        let m = set.find(text, None);
        assert_eq!(m.len(), 1);
        assert_eq!(&text[m[0].start..m[0].end], "abcdeé");
    }

    #[test]
    fn vendored_rules_match_their_notice() {
        let digest = hex::encode(Sha256::digest(GITLEAKS_TOML.as_bytes()));
        assert_eq!(Some(digest.as_str()), notice_field("sha256"));
        assert!(notice_field("version").is_some_and(|v| v.starts_with('v')));
        let set = imported();
        assert!(set.rules().len() >= 200, "{}", set.rules().len());
        // Every rule has a distinct placeholder label.
        let mut labels: Vec<&str> = set.rules().iter().map(Rule::label).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), set.rules().len());
        assert!(set.by_label("aws_access_token").is_some());
    }
}
