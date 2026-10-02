//! `BETWEEN SYMMETRIC` and `ANY` / `ALL` over a subquery (PostgreSQL).

use sqlparser::ast::Statement;
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

#[path = "support/quantified.rs"]
mod quantified;

fn parse_one(sql: &str) -> Statement {
    let mut stmts = Parser::parse_sql(&PostgreSqlDialect {}, sql)
        .unwrap_or_else(|e| panic!("{sql:?} does not parse: {e}"));
    assert_eq!(stmts.len(), 1, "{sql:?}");
    let stmt = stmts.pop().unwrap();
    let again = Parser::parse_sql(&PostgreSqlDialect {}, &stmt.to_string())
        .unwrap_or_else(|e| panic!("rendering {stmt} does not parse: {e}"));
    assert_eq!(
        again,
        vec![stmt.clone()],
        "rendering {stmt} changes the statement"
    );
    stmt
}

fn round_trips(sql: &str) {
    assert_eq!(parse_one(sql).to_string(), sql);
}

/// The rendering must retain a subquery operand. PostgreSQL accepts redundant
/// parentheses around it, including for row comparisons and WITH queries.
fn assert_subquery_operand(rendered: &str, keyword: &str) {
    use sqlparser::tokenizer::Token;
    let tokens = quantified::normalized(rendered);
    assert!(
        tokens.windows(3).any(|w| {
            matches!(&w[0], Token::Word(k) if k.quote_style.is_none() && k.value == keyword)
                && w[1] == Token::LParen
                && matches!(&w[2], Token::Word(k) if k.quote_style.is_none() && matches!(k.value.as_str(), "SELECT" | "WITH"))
        }),
        "{rendered}"
    );
}

#[test]
fn between_symmetric_round_trips() {
    round_trips("SELECT order_id FROM orders WHERE total_cents BETWEEN SYMMETRIC 50000 AND 20000");
}

#[test]
fn not_between_symmetric_round_trips() {
    round_trips("SELECT order_id FROM orders WHERE total_cents NOT BETWEEN SYMMETRIC 100 AND 999999 ORDER BY total_cents DESC");
}

#[test]
fn symmetric_is_kept_in_the_tree() {
    let symmetric = parse_one("SELECT x FROM t WHERE y BETWEEN SYMMETRIC 1 AND 2");
    let plain = parse_one("SELECT x FROM t WHERE y BETWEEN 1 AND 2");
    assert_ne!(symmetric, plain);
}

#[test]
fn between_symmetric_inside_a_conjunction() {
    let stmt = parse_one(
        "SELECT e.event_id FROM events e WHERE e.created_at BETWEEN SYMMETRIC now() AND now() - INTERVAL '1 day' AND e.account_id = 7",
    );
    let rendered = stmt.to_string();
    assert!(
        rendered
            .contains("BETWEEN SYMMETRIC now() AND now() - INTERVAL '1 day' AND e.account_id = 7"),
        "{rendered}"
    );
}

#[test]
fn between_asymmetric_is_accepted() {
    parse_one("SELECT x FROM t WHERE y BETWEEN ASYMMETRIC 1 AND 2");
}

#[test]
fn any_over_a_subquery() {
    quantified::check_normalization();
    let stmt = parse_one(
        "SELECT r.refund_id FROM refunds r WHERE r.order_id = ANY (SELECT o.order_id FROM orders o WHERE o.customer_id = 48213)",
    );
    assert_subquery_operand(&stmt.to_string(), "ANY");
}

#[test]
fn all_over_a_subquery_with_a_cte() {
    let stmt = parse_one(
        "SELECT i.invoice_id FROM invoices i WHERE i.customer_id <> ALL (WITH blocked AS (SELECT customer_id FROM blocklist) SELECT customer_id FROM blocked)",
    );
    assert_subquery_operand(&stmt.to_string(), "ALL");
}

#[test]
fn row_compared_with_any_subquery() {
    let stmt = parse_one(
        "SELECT r.refund_id FROM refunds r WHERE (r.order_id, r.customer_id) = ANY (SELECT o.order_id, o.customer_id FROM orders o)",
    );
    assert_subquery_operand(&stmt.to_string(), "ANY");
}

#[test]
fn any_over_an_array_is_unchanged() {
    let stmt =
        parse_one("SELECT t.ticket_id FROM support_tickets t WHERE t.priority = ANY (ARRAY[1, 2])");
    assert_eq!(
        stmt.to_string(),
        "SELECT t.ticket_id FROM support_tickets AS t WHERE t.priority = ANY(ARRAY[1, 2])"
    );
}
