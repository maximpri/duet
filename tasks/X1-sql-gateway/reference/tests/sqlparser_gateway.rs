//! Gateway regressions: PostgreSQL 16 constants, BETWEEN SYMMETRIC, ANY/ALL
//! over subqueries, lock strengths and statement fingerprints.

use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::fingerprint::fingerprint;
use sqlparser::parser::Parser;

fn render(sql: &str) -> String {
    let stmts = Parser::parse_sql(&PostgreSqlDialect {}, sql).unwrap();
    stmts.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("; ")
}

#[test]
fn postgres_numeric_constants() {
    assert_eq!(render("SELECT 1_000, 0x1F, 0o_17, 0b1_0"), "SELECT 1_000, 0x1F, 0o_17, 0b1_0");
    assert_eq!(render("SELECT a - 25_000 FROM t"), "SELECT a - 25_000 FROM t");
}

#[test]
fn continued_string_constants() {
    assert_eq!(render("SELECT 'a'\n'b' AS x"), "SELECT 'ab' AS x");
    assert_eq!(render("SELECT 'a' -- c\n  'b'"), "SELECT 'ab'");
}

#[test]
fn symmetric_between_any_all_and_locks() {
    for sql in [
        "SELECT x FROM t WHERE y NOT BETWEEN SYMMETRIC 1 AND 2",
        "SELECT x FROM t WHERE y = ANY(SELECT z FROM u)",
        "SELECT x FROM t WHERE y <> ALL(SELECT z FROM u)",
        "SELECT x FROM t FOR NO KEY UPDATE OF t NOWAIT",
        "SELECT x FROM t FOR KEY SHARE SKIP LOCKED",
    ] {
        assert_eq!(render(sql), sql);
    }
}

#[test]
fn fingerprints() {
    let fp = |s| fingerprint(&PostgreSqlDialect {}, s).unwrap();
    assert_eq!(fp("SELECT a - 1, -2 FROM t WHERE id IN (1, 2) -- x"), "SELECT a - ?, ? FROM t WHERE id IN (?)");
    assert_eq!(fp("INSERT INTO t VALUES (1), (2); ;"), "INSERT INTO t VALUES (?)");
}
