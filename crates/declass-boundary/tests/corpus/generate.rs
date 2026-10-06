// SPDX-License-Identifier: GPL-3.0-or-later
//! Synthetic text that matches a regular expression, for the positives of the
//! detection corpus: literals as written, one member of each class (letters
//! and digits drawn at random, so random-looking parts look random), a count
//! for each repetition, one alternative. The first attempt takes the plainest
//! path (the first alternative, the fewest optional characters, separators as
//! spaces); later attempts vary it. Every character comes from a seeded
//! generator: nothing here is, or was derived from, a real credential.

use regex_syntax::hir::{Class, Hir, HirKind, Repetition};

/// Characters drawn for a repetition that has room for them: enough for the
/// entropy thresholds of the rules (3 to 4.5 bits per character).
const RANDOM_RUN: u32 = 24;

pub struct Sampler {
    state: u64,
    attempt: u32,
}

impl Sampler {
    pub fn new(seed: u64, attempt: u32) -> Self {
        let mut s = Self {
            state: (seed ^ 0x9E37_79B9_7F4A_7C15).wrapping_add(u64::from(attempt) << 32) | 1,
            attempt,
        };
        s.next();
        s
    }

    fn next(&mut self) -> u64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    /// A string `pattern` (RE2 syntax, as in the rule file) matches, or `None`
    /// when the pattern does not parse.
    pub fn sample(&mut self, pattern: &str) -> Option<String> {
        let translated = declass_boundary::rules::translate(pattern);
        let hir = [false, true].into_iter().find_map(|unicode| {
            regex_syntax::ParserBuilder::new()
                .unicode(unicode)
                .utf8(false)
                .build()
                .parse(&translated)
                .ok()
        })?;
        let mut out = Vec::new();
        self.walk(&hir, &mut out);
        String::from_utf8(out).ok()
    }

    fn walk(&mut self, h: &Hir, out: &mut Vec<u8>) {
        match h.kind() {
            HirKind::Empty | HirKind::Look(_) => {}
            HirKind::Literal(l) => out.extend_from_slice(&l.0),
            HirKind::Class(c) => {
                if let Some(b) = self.pick(c) {
                    out.push(b);
                }
            }
            HirKind::Repetition(r) => {
                for _ in 0..self.count(r) {
                    self.walk(&r.sub, out);
                }
            }
            HirKind::Capture(c) => self.walk(&c.sub, out),
            HirKind::Concat(parts) => {
                for p in parts {
                    self.walk(p, out);
                }
            }
            HirKind::Alternation(alts) => {
                let i = if self.attempt == 0 {
                    0
                } else {
                    self.below(alts.len())
                };
                self.walk(&alts[i], out);
            }
        }
    }

    /// A printable member of the class: a letter or digit at random when it
    /// has any (a space first for separator classes), else any member.
    fn pick(&mut self, class: &Class) -> Option<u8> {
        let mut members: Vec<u8> = Vec::new();
        let mut add = |lo: u32, hi: u32| {
            for b in lo.max(0x09)..=hi.min(0x7E) {
                let b = b as u8;
                if b.is_ascii_graphic() || b == b' ' || b == b'\t' || b == b'\n' {
                    members.push(b);
                }
            }
        };
        match class {
            Class::Bytes(c) => c
                .ranges()
                .iter()
                .for_each(|r| add(u32::from(r.start()), u32::from(r.end()))),
            Class::Unicode(c) => c
                .ranges()
                .iter()
                .for_each(|r| add(r.start() as u32, r.end() as u32)),
        }
        let alnum: Vec<u8> = members
            .iter()
            .copied()
            .filter(u8::is_ascii_alphanumeric)
            .collect();
        let printable: Vec<u8> = members
            .iter()
            .copied()
            .filter(|b| b.is_ascii_graphic() || *b == b' ')
            .collect();
        // A case-insensitive letter (`[Aa]`) is written in lower case.
        if let [upper, lower] = members[..]
            && upper.to_ascii_lowercase() == lower
            && upper != lower
        {
            return Some(lower);
        }
        if !alnum.is_empty() && (self.attempt == 0 || self.below(5) != 0) {
            Some(alnum[self.below(alnum.len())])
        } else if printable.contains(&b' ') && self.attempt == 0 {
            Some(b' ')
        } else if !printable.is_empty() {
            Some(printable[self.below(printable.len())])
        } else {
            members.first().copied()
        }
    }

    fn count(&mut self, r: &Repetition) -> u32 {
        let max = r.max.unwrap_or((r.min + 8).max(RANDOM_RUN));
        if self.attempt == 0 {
            if r.min == 0 || max == r.min || r.min >= RANDOM_RUN {
                r.min
            } else {
                RANDOM_RUN.clamp(r.min, max)
            }
        } else {
            let hi = max.min(r.min + 40);
            r.min + self.below((hi - r.min + 1) as usize) as u32
        }
    }
}

/// A stable seed for a rule id.
pub fn seed(id: &str) -> u64 {
    id.bytes().fold(0xCBF2_9CE4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01B3)
    })
}
