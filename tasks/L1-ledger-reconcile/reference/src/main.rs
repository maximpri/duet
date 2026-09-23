//! `ledger <data-dir> <YYYY-MM>`: reconcile and print the variance report.

use ledger::entries::parse_ledger;
use ledger::reconcile::reconcile;
use ledger::report::variance_report;
use ledger::statement::parse_statement;
use std::fs;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, month) = match args.as_slice() {
        [_, dir, month] => (Path::new(dir), month.as_str()),
        _ => {
            eprintln!("usage: ledger <data-dir> <YYYY-MM>");
            std::process::exit(2);
        }
    };
    let mut statements = Vec::new();
    let mut files: Vec<_> = fs::read_dir(dir.join("statements")).expect("statements dir").flatten().collect();
    files.sort_by_key(|e| e.file_name());
    for entry in files {
        let name = entry.file_name().to_string_lossy().into_owned();
        let content = fs::read_to_string(entry.path()).expect("read statement");
        match parse_statement(&name, &content) {
            Ok(s) => statements.push(s),
            Err(e) => eprintln!("statement {name} rejected: {e:?}"),
        }
    }
    let entries = parse_ledger(&fs::read_to_string(dir.join("ledger.tsv")).expect("ledger")).expect("parse ledger");
    let rec = reconcile(&statements, &entries);
    println!("matched {}, unmatched ledger {}, unmatched bank {}", rec.matched.len(), rec.unmatched_ledger.len(), rec.unmatched_bank.len());
    let accounts = fs::read_to_string(dir.join("accounts.csv")).expect("accounts");
    match variance_report(&statements, &entries, &accounts, month) {
        Ok(rows) => {
            for r in rows {
                println!("{:<12} ledger {:>12} bank {:>12} variance {:>10}", r.account, r.ledger_cents, r.bank_cents, r.variance_cents);
            }
        }
        Err(e) => eprintln!("report failed: {e:?}"),
    }
}
