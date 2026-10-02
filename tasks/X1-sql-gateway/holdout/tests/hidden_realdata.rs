//! The September audit log and the scheduled reports: every statement parses,
//! its rendering (what the gateway forwards) is stable, and fingerprints match
//! the reviewed baseline in tests/realdata/.

use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::fingerprint::fingerprint;
use sqlparser::parser::Parser;
use std::fs;

#[path = "support/quantified.rs"]
#[allow(dead_code)]
mod quantified;

const LOG: &str = "logs/gateway-audit-2026-09.log";
const REPORTS: &str = "data/scheduled_reports.sql";

/// The `statement:` blocks of the audit log, in order.
fn log_statements() -> Vec<String> {
    let text = fs::read_to_string(LOG).expect("audit log");
    let mut out = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line != "    statement:" {
            continue;
        }
        let mut stmt = Vec::new();
        while let Some(next) = lines.peek() {
            if let Some(rest) = next.strip_prefix("      | ") {
                stmt.push(rest.to_string());
            } else if *next == "      |" {
                stmt.push(String::new());
            } else {
                break;
            }
            lines.next();
        }
        out.push(stmt.join("\n"));
    }
    out
}

/// One block per `-- report:` header, header included.
fn report_statements() -> Vec<String> {
    let text = fs::read_to_string(REPORTS).expect("scheduled reports");
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.starts_with("-- report:") {
            out.push(String::new());
        }
        if let Some(block) = out.last_mut() {
            block.push_str(line);
            block.push('\n');
        }
    }
    out
}

fn expected(name: &str) -> Vec<(String, String)> {
    fs::read_to_string(format!("tests/realdata/{name}"))
        .expect("baseline")
        .lines()
        .map(|l| {
            let (a, b) = l.split_once('\t').expect("two columns");
            (a.to_string(), b.to_string())
        })
        .collect()
}

fn render(sql: &str) -> Result<String, String> {
    let stmts = Parser::parse_sql(&PostgreSqlDialect {}, sql).map_err(|e| e.to_string())?;
    Ok(stmts
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join("; "))
}

fn check_parse(stmts: &[String]) {
    let failures: Vec<String> = stmts
        .iter()
        .enumerate()
        .filter_map(|(i, s)| render(s).err().map(|e| format!("#{i}: {e}")))
        .collect();
    assert!(
        failures.is_empty(),
        "{} statements do not parse:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn check_stable(stmts: &[String]) {
    let mut failures = Vec::new();
    for (i, s) in stmts.iter().enumerate() {
        let Ok(first) = Parser::parse_sql(&PostgreSqlDialect {}, s) else {
            failures.push(format!("#{i}: does not parse"));
            continue;
        };
        let rendered = first
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        match Parser::parse_sql(&PostgreSqlDialect {}, &rendered) {
            Ok(second) if second == first => {}
            Ok(_) => failures.push(format!("#{i}: rendering parses differently")),
            Err(e) => failures.push(format!("#{i}: rendering does not parse: {e}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn check_fingerprints(stmts: &[String], baseline: &[(String, String)]) {
    assert_eq!(stmts.len(), baseline.len(), "number of statements");
    let mut failures = Vec::new();
    for (i, (s, (want, _))) in stmts.iter().zip(baseline).enumerate() {
        match fingerprint(&PostgreSqlDialect {}, s) {
            Ok(got) if &got == want => {}
            Ok(got) => failures.push(format!("#{i}: got {got}\n      want {want}")),
            Err(e) => failures.push(format!("#{i}: {e}")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn check_rendering_fingerprints(stmts: &[String], baseline: &[(String, String)]) {
    assert_eq!(stmts.len(), baseline.len(), "number of statements");
    let mut failures = Vec::new();
    for (i, (s, (_, want))) in stmts.iter().zip(baseline).enumerate() {
        let Ok(rendered) = render(s) else {
            failures.push(format!("#{i}: does not parse"));
            continue;
        };
        match fingerprint(&PostgreSqlDialect {}, &rendered) {
            Ok(got) if quantified::normalized(&got) == quantified::normalized(want) => {}
            Ok(got) => failures.push(format!("#{i}: forwarded {got}\n      want {want}")),
            Err(e) => failures.push(format!("#{i}: {e}")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn log_statements_parse() {
    check_parse(&log_statements());
}

#[test]
fn log_renderings_are_stable() {
    check_stable(&log_statements());
}

#[test]
fn log_fingerprints_match_the_baseline() {
    check_fingerprints(&log_statements(), &expected("log.tsv"));
}

#[test]
fn log_forwarded_statements_keep_their_meaning() {
    check_rendering_fingerprints(&log_statements(), &expected("log.tsv"));
}

#[test]
fn reports_parse() {
    check_parse(&report_statements());
}

#[test]
fn report_renderings_are_stable() {
    check_stable(&report_statements());
}

#[test]
fn report_fingerprints_match_the_baseline() {
    check_fingerprints(&report_statements(), &expected("reports.tsv"));
}

#[test]
fn forwarded_reports_keep_their_meaning() {
    check_rendering_fingerprints(&report_statements(), &expected("reports.tsv"));
}
