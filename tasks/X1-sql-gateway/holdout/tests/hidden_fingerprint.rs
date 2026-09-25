//! `sqlparser::fingerprint::fingerprint`, as specified in docs/FINGERPRINT.md.

use sqlparser::dialect::{Dialect, MySqlDialect, PostgreSqlDialect};
use sqlparser::fingerprint::fingerprint;

fn fp_in(dialect: &dyn Dialect, sql: &str) -> String {
    fingerprint(dialect, sql).unwrap_or_else(|e| panic!("{sql:?}: {e}"))
}

fn fp(sql: &str) -> String {
    fp_in(&PostgreSqlDialect {}, sql)
}

#[test]
fn constants_keywords_and_identifiers() {
    assert_eq!(
        fp("select Name from Customers where email = 'x@example.com'"),
        "SELECT NAME FROM customers WHERE email = ?"
    );
}

#[test]
fn comments_are_dropped_and_in_lists_collapse() {
    assert_eq!(
        fp("SELECT count(*) FROM orders WHERE id IN (1, 2, 3) -- ticket 7"),
        "SELECT COUNT (*) FROM orders WHERE id IN (?)"
    );
    assert_eq!(
        fp("/* asked by a customer */ DELETE FROM sessions WHERE id NOT IN ('a', 'b') AND x IN (1, y)"),
        "DELETE FROM sessions WHERE id NOT IN (?) AND x IN (?, y)"
    );
}

#[test]
fn signs_after_operators_and_listed_keywords_belong_to_the_constant() {
    assert_eq!(fp("SELECT a - 1, -1 FROM t LIMIT 10"), "SELECT a - ?, ? FROM t LIMIT ?");
    assert_eq!(
        fp("SELECT x::int - 4, CASE WHEN a THEN -1 ELSE +2 END - 3, f(-1, -2), 5 * -2 FROM t WHERE b = -7"),
        "SELECT x::INT - ?, CASE WHEN a THEN ? ELSE ? END - ?, f (?, ?), ? * ? FROM t WHERE b = ?"
    );
}

#[test]
fn quoted_identifiers_and_placeholders_are_kept() {
    assert_eq!(
        fp(r#"SELECT "Kunde"."Name" FROM "Kunde" WHERE id = $1 AND tag IN ($2, $3)"#),
        r#"SELECT "Kunde"."Name" FROM "Kunde" WHERE id = $1 AND tag IN ($2, $3)"#
    );
}

#[test]
fn repeated_values_rows_collapse() {
    assert_eq!(
        fp("INSERT INTO t (a, b) VALUES (1, 'x'), (2, 'y')"),
        "INSERT INTO t (a, b) VALUES (?, ?)"
    );
    assert_eq!(
        fp("INSERT INTO t VALUES (1, 2), (3, DEFAULT), (4, 5), (6, 7) RETURNING id"),
        "INSERT INTO t VALUES (?, ?), (?, DEFAULT), (?, ?) RETURNING id"
    );
}

#[test]
fn statements_are_joined_and_empty_ones_dropped() {
    assert_eq!(fp("SELECT x::text FROM t; SELECT 1;"), "SELECT x::TEXT FROM t; SELECT ?");
    assert_eq!(fp(";; SELECT 1 ;; ; SELECT 2"), "SELECT ?; SELECT ?");
}

#[test]
fn every_kind_of_string_constant() {
    assert_eq!(
        fp(r"SELECT E'it\'s', $$body$$, $tag$x$tag$, X'1F', N'abc', 'plain', DATE '2026-09-01', INTERVAL '7 days'"),
        "SELECT ?, ?, ?, ?, ?, ?, DATE ?, INTERVAL ?"
    );
}

#[test]
fn arrays_of_constants_collapse() {
    assert_eq!(
        fp("SELECT ARRAY[1, 2, 3], ARRAY[1, x], ARRAY['a']"),
        "SELECT ARRAY [?], ARRAY [?, x], ARRAY [?]"
    );
}

#[test]
fn a_continued_string_is_one_constant() {
    assert_eq!(fp("SELECT 'Dear customer, '\n       'your order shipped' AS body"), "SELECT ? AS body");
}

#[test]
fn postgres_numbers_are_single_constants() {
    assert_eq!(
        fp("SELECT 1_000_000, 0x1F, -0b101, 1_000.5 FROM t WHERE flags & 0x04 <> 0"),
        "SELECT ?, ?, ?, ? FROM t WHERE flags & ? <> ?"
    );
}

#[test]
fn statements_that_do_not_parse_still_have_a_fingerprint() {
    assert_eq!(fp("SELECT FROM WHERE 'x' (("), "SELECT FROM WHERE ? ((");
}

#[test]
fn tokenizer_errors_are_reported() {
    assert!(fingerprint(&PostgreSqlDialect {}, "SELECT 'unterminated").is_err());
}

#[test]
fn no_constant_text_survives() {
    let sql = "INSERT INTO customers (full_name, email, phone) VALUES ('Ines Quenarmont', 'ines.q@mailbox-417.net', '+1 (555) 201-7788') RETURNING customer_id";
    let out = fp(sql);
    assert_eq!(out, "INSERT INTO customers (full_name, email, phone) VALUES (?, ?, ?) RETURNING customer_id");
    for secret in ["Ines", "mailbox", "555", "7788"] {
        assert!(!out.contains(secret), "{out}");
    }
}

#[test]
fn double_quotes_follow_the_dialect() {
    assert_eq!(fp_in(&MySqlDialect {}, r#"SELECT "abc" FROM t"#), "SELECT ? FROM t");
    assert_eq!(fp(r#"SELECT "abc" FROM t"#), r#"SELECT "abc" FROM t"#);
}
