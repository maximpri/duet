// SPDX-License-Identifier: GPL-3.0-or-later
//! PostgreSQL treats extra parentheses around an ANY/ALL subquery as part
//! of that subquery, not as a scalar expression yielding an array. Normalize
//! only those redundant wrappers when comparing renderings. The fingerprint
//! API itself must still preserve every token, as its specification requires.

use sqlparser::dialect::GenericDialect;
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Token, Tokenizer};

pub fn normalized(sql: &str) -> Vec<Token> {
    // The generic tokenizer keeps a fingerprint's `?` placeholders intact;
    // PostgreSQL's JSON `?` operator is unrelated to this comparison.
    let mut tokens: Vec<_> = Tokenizer::new(&GenericDialect {}, sql)
        .tokenize()
        .expect("tokenize rendering or fingerprint")
        .into_iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    let mut i = 0;
    while i < tokens.len() {
        if matches!(&tokens[i], Token::Word(w)
            if w.quote_style.is_none() && matches!(w.keyword, Keyword::ANY | Keyword::ALL | Keyword::SOME))
        {
            let outer = i + 1;
            while tokens.get(outer) == Some(&Token::LParen)
                && tokens.get(outer + 1) == Some(&Token::LParen)
            {
                let mut start = outer + 1;
                while tokens.get(start) == Some(&Token::LParen) {
                    start += 1;
                }
                if !matches!(tokens.get(start), Some(Token::Word(w))
                    if w.quote_style.is_none() && matches!(w.keyword, Keyword::SELECT | Keyword::WITH | Keyword::VALUES))
                {
                    break;
                }
                let Some(end) = closing(&tokens, outer) else {
                    break;
                };
                // Both parentheses must wrap the entire query. Keep casts,
                // operators, array expressions and set-operation grouping.
                if closing(&tokens, outer + 1) != end.checked_sub(1) {
                    break;
                }
                tokens.remove(end - 1);
                tokens.remove(outer + 1);
            }
        }
        i += 1;
    }
    tokens
}

fn closing(tokens: &[Token], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, token) in tokens.iter().enumerate().skip(start) {
        match token {
            Token::LParen => depth += 1,
            Token::RParen => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Check the comparison helper as part of the existing predicate test, so
/// grader self-checks do not inflate the candidate's hidden-test denominator.
pub fn check_normalization() {
    for (plain, wrapped) in [
        ("x = ANY(SELECT y FROM t)", "x = ANY(((SELECT y FROM t)))"),
        (
            "x <> ALL(WITH t AS (SELECT 1) SELECT * FROM t)",
            "x <> ALL((WITH t AS (SELECT 1) SELECT * FROM t))",
        ),
        (
            "(a,b) = ANY(SELECT x,y FROM t)",
            "(a,b) = ANY((SELECT x,y FROM t))",
        ),
        ("x = ANY(VALUES (1),(2))", "x = ANY((VALUES (1),(2)))"),
        (
            "x = ANY(SELECT y FROM t WHERE z = ANY(SELECT z FROM u))",
            "x = ANY((SELECT y FROM t WHERE z = ANY((SELECT z FROM u))))",
        ),
    ] {
        assert_eq!(normalized(plain), normalized(wrapped), "{wrapped}");
    }
    for (a, b) in [
        (
            "x = ANY((SELECT y FROM t)::int[])",
            "x = ANY(SELECT y FROM t)",
        ),
        ("x = ANY((SELECT y FROM t) + 1)", "x = ANY(SELECT y FROM t)"),
        ("x = ANY(ARRAY[1,2])", "x = ANY(SELECT y FROM t)"),
        (
            "x = ANY((SELECT 1 LIMIT 1) UNION SELECT 2)",
            "x = ANY(SELECT 1 LIMIT 1 UNION SELECT 2)",
        ),
        ("x = ANY(SELECT 1)", "x = ALL(SELECT 1)"),
        ("SELECT 'ANY((SELECT x))'", "SELECT 'ANY(SELECT x)'"),
        ("SELECT \"ANY((SELECT x))\"", "SELECT \"ANY(SELECT x)\""),
    ] {
        assert_ne!(normalized(a), normalized(b), "{a}");
    }
}
