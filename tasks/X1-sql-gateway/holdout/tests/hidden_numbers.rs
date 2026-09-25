//! PostgreSQL 16 numeric constants: underscores between digits and
//! non-decimal integers are numbers.

use sqlparser::ast::{Expr, SelectItem, SetExpr, Statement, Value};
use sqlparser::dialect::{MySqlDialect, PostgreSqlDialect};
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

/// The integer value of a numeric literal in any accepted spelling.
fn number_value(n: &str) -> String {
    let n = n.replace('_', "");
    let lower = n.to_ascii_lowercase();
    for (prefix, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] {
        if let Some(digits) = lower.strip_prefix(prefix) {
            return u64::from_str_radix(digits, radix)
                .unwrap_or_else(|e| panic!("{n}: {e}"))
                .to_string();
        }
    }
    n
}

fn unnamed_number(item: &SelectItem) -> String {
    match item {
        SelectItem::UnnamedExpr(Expr::Value(Value::Number(n, _))) => number_value(n),
        other => panic!("expected a single number without alias, got {other}"),
    }
}

#[test]
fn underscores_between_digits_make_one_number() {
    let stmt = parse_one("SELECT 1_500_000");
    let items = projection(&stmt);
    assert_eq!(items.len(), 1, "{stmt}");
    assert_eq!(unnamed_number(&items[0]), "1500000");
}

#[test]
fn underscores_in_decimals_and_exponents() {
    let stmt = parse_one("SELECT 1_000.000_1, 6.022_140e2_3, .5_5");
    let items = projection(&stmt);
    assert_eq!(items.len(), 3, "{stmt}");
    assert_eq!(unnamed_number(&items[0]), "1000.0001");
    assert_eq!(unnamed_number(&items[1]).to_ascii_lowercase(), "6.022140e23");
    assert_eq!(unnamed_number(&items[2]), ".55");
}

#[test]
fn underscore_number_inside_an_expression_is_not_an_alias() {
    let stmt = parse_one(
        "SELECT a.customer_id, a.credit_limit_cents - 25_000 FROM accounts a WHERE a.owner_email = 'x'",
    );
    let items = projection(&stmt);
    assert_eq!(items.len(), 2, "{stmt}");
    match &items[1] {
        SelectItem::UnnamedExpr(Expr::BinaryOp { right, .. }) => match right.as_ref() {
            Expr::Value(Value::Number(n, _)) => assert_eq!(number_value(n), "25000"),
            other => panic!("right operand is {other}"),
        },
        other => panic!("expected `credit_limit_cents - 25000` without alias, got {other}"),
    }
}

#[test]
fn underscore_number_as_limit() {
    let stmt = parse_one("SELECT customer_id FROM customers ORDER BY created_at DESC LIMIT 1_000");
    match &stmt {
        Statement::Query(q) => match &q.limit {
            Some(Expr::Value(Value::Number(n, _))) => assert_eq!(number_value(n), "1000"),
            other => panic!("limit is {other:?}"),
        },
        other => panic!("not a query: {other}"),
    }
}

#[test]
fn non_decimal_integers_are_numbers() {
    let stmt = parse_one("SELECT 0x1F, 0o17, 0b101, 0XFF");
    let items = projection(&stmt);
    assert_eq!(items.len(), 4, "{stmt}");
    let values: Vec<String> = items.iter().map(unnamed_number).collect();
    assert_eq!(values, ["31", "15", "5", "255"]);
}

#[test]
fn non_decimal_integers_with_underscores() {
    let stmt = parse_one("SELECT 0xFFFF_FFFF, 0o_1_755, 0b1000_0000, 0x_0100");
    let items = projection(&stmt);
    assert_eq!(items.len(), 4, "{stmt}");
    let values: Vec<String> = items.iter().map(unnamed_number).collect();
    assert_eq!(values, ["4294967295", "1005", "128", "256"]);
}

#[test]
fn hex_integer_in_a_bit_mask_filter_stays_an_integer() {
    let stmt = parse_one("SELECT c.customer_id FROM customers c WHERE c.flags & 0x04 <> 0");
    let rendered = stmt.to_string();
    assert!(!rendered.contains("X'"), "rendered as a bit string: {rendered}");
    match &stmt {
        Statement::Query(q) => match q.body.as_ref() {
            SetExpr::Select(s) => match &s.selection {
                Some(Expr::BinaryOp { left, .. }) => match left.as_ref() {
                    Expr::BinaryOp { right, .. } => match right.as_ref() {
                        Expr::Value(Value::Number(n, _)) => assert_eq!(number_value(n), "4"),
                        other => panic!("mask is {other}"),
                    },
                    other => panic!("left side is {other}"),
                },
                other => panic!("selection is {other:?}"),
            },
            _ => unreachable!(),
        },
        _ => unreachable!(),
    }
}

#[test]
fn mysql_keeps_hexadecimal_string_literals() {
    let stmts = Parser::parse_sql(&MySqlDialect {}, "SELECT 0x1F").unwrap();
    assert_eq!(stmts[0].to_string(), "SELECT X'1F'");
}
