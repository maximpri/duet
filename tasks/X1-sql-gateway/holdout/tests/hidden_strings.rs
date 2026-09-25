//! PostgreSQL string constant continuation: constants separated only by
//! whitespace containing a newline are one constant.

use sqlparser::ast::{Expr, SelectItem, SetExpr, Statement, Value};
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

fn projection(stmt: &Statement) -> Vec<SelectItem> {
    match stmt {
        Statement::Query(q) => match q.body.as_ref() {
            SetExpr::Select(s) => s.projection.clone(),
            other => panic!("not a SELECT: {other}"),
        },
        other => panic!("not a query: {other}"),
    }
}

fn string_of(expr: &Expr) -> String {
    match expr {
        Expr::Value(Value::SingleQuotedString(s)) => s.clone(),
        other => panic!("expected one string constant, got {other}"),
    }
}

#[test]
fn two_lines_are_one_constant() {
    let stmt = parse_one(
        "SELECT c.customer_id, 'Payment reminder: invoice overdue, '\n       'please contact billing'\nFROM customers c",
    );
    let items = projection(&stmt);
    assert_eq!(items.len(), 2, "{stmt}");
    match &items[1] {
        SelectItem::UnnamedExpr(e) => {
            assert_eq!(string_of(e), "Payment reminder: invoice overdue, please contact billing")
        }
        other => panic!("expected an unnamed string, got {other}"),
    }
}

#[test]
fn three_parts_with_an_alias() {
    let stmt = parse_one("SELECT 'a'\n  'b'\n  'c' AS x");
    let items = projection(&stmt);
    assert_eq!(items.len(), 1, "{stmt}");
    match &items[0] {
        SelectItem::ExprWithAlias { expr, alias } => {
            assert_eq!(string_of(expr), "abc");
            assert_eq!(alias.value, "x");
        }
        other => panic!("expected 'abc' AS x, got {other}"),
    }
}

#[test]
fn continuation_in_an_update() {
    let stmt = parse_one(
        "UPDATE support_tickets\nSET resolution = 'Refund issued after courier lost the parcel; '\n                 'customer accepted a voucher.'\nWHERE ticket_id = 88213",
    );
    let rendered = stmt.to_string();
    assert!(
        rendered.contains("'Refund issued after courier lost the parcel; customer accepted a voucher.'"),
        "{rendered}"
    );
}

#[test]
fn continuation_across_a_line_comment() {
    let stmt = parse_one("SELECT 'foo' -- first part\n'bar'");
    let items = projection(&stmt);
    assert_eq!(items.len(), 1, "{stmt}");
    match &items[0] {
        SelectItem::UnnamedExpr(e) => assert_eq!(string_of(e), "foobar"),
        other => panic!("expected 'foobar', got {other}"),
    }
}

#[test]
fn doubled_quotes_survive_continuation() {
    let stmt = parse_one("SELECT 'O''Hara'\n' Ltd' AS company");
    let items = projection(&stmt);
    match &items[0] {
        SelectItem::ExprWithAlias { expr, .. } => assert_eq!(string_of(expr), "O'Hara Ltd"),
        other => panic!("expected a string with an alias, got {other}"),
    }
    assert_eq!(stmt.to_string(), "SELECT 'O''Hara Ltd' AS company");
}

#[test]
fn separate_constants_stay_separate() {
    let stmt = parse_one("SELECT 'a',\n'b'");
    assert_eq!(projection(&stmt).len(), 2, "{stmt}");
}
