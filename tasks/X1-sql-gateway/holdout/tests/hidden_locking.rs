//! PostgreSQL row-level lock strengths `FOR NO KEY UPDATE` and `FOR KEY SHARE`.

use sqlparser::ast::Statement;
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

fn parse_one(sql: &str) -> Statement {
    let mut stmts = Parser::parse_sql(&PostgreSqlDialect {}, sql)
        .unwrap_or_else(|e| panic!("{sql:?} does not parse: {e}"));
    assert_eq!(stmts.len(), 1, "{sql:?}");
    let stmt = stmts.pop().unwrap();
    let again = Parser::parse_sql(&PostgreSqlDialect {}, &stmt.to_string())
        .unwrap_or_else(|e| panic!("rendering {stmt} does not parse: {e}"));
    assert_eq!(again, vec![stmt.clone()], "rendering {stmt} changes the statement");
    stmt
}

fn round_trips(sql: &str) {
    assert_eq!(parse_one(sql).to_string(), sql);
}

#[test]
fn for_no_key_update() {
    round_trips("SELECT balance_cents FROM wallets WHERE customer_id = 48213 FOR NO KEY UPDATE");
}

#[test]
fn for_key_share_of_a_table_nowait() {
    round_trips("SELECT w.balance_cents FROM wallets AS w WHERE w.customer_id = 51190 FOR KEY SHARE OF w NOWAIT");
}

#[test]
fn for_key_share_skip_locked() {
    round_trips("SELECT w.customer_id FROM wallets AS w WHERE w.balance_cents > 0 FOR KEY SHARE SKIP LOCKED");
}

#[test]
fn several_lock_clauses() {
    round_trips("SELECT * FROM t JOIN u ON t.id = u.id FOR KEY SHARE OF t FOR NO KEY UPDATE OF u SKIP LOCKED");
}

#[test]
fn lock_strengths_are_distinct() {
    let strengths = [
        parse_one("SELECT * FROM t FOR UPDATE"),
        parse_one("SELECT * FROM t FOR NO KEY UPDATE"),
        parse_one("SELECT * FROM t FOR SHARE"),
        parse_one("SELECT * FROM t FOR KEY SHARE"),
    ];
    for i in 0..strengths.len() {
        for j in 0..i {
            assert_ne!(strengths[i], strengths[j]);
        }
    }
}
