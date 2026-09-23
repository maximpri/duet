use ledger::date::Date;
use ledger::entries::parse_ledger;
use ledger::reconcile::reconcile;
use ledger::report::{variance_report, AccountVariance, ReportError};
use ledger::statement::parse_statement;
use ledger::ParseError;

const EU: &str = "Buchungsdatum;Verwendungszweck;Betrag;Referenz\n";
const US: &str = "date,description,amount,reference\n";
const LH: &str = "entry_id\tdate\taccount\tcounterparty\treference\tamount\n";

fn us(rows: &str) -> ledger::statement::Statement {
    parse_statement("bank_b_2026-08.csv", &format!("{US}{rows}")).unwrap()
}
fn eu(rows: &str) -> ledger::statement::Statement {
    parse_statement("bank_a_2026-08.csv", &format!("{EU}{rows}")).unwrap()
}
fn led(rows: &str) -> Vec<ledger::entries::Entry> {
    parse_ledger(&format!("{LH}{rows}")).unwrap()
}

#[test]
fn us_statement_still_parses() {
    let s = us("2026-08-02,Fee,-12.50,FEE-1\n");
    assert_eq!(s.lines[0].amount_cents, -1250);
    assert_eq!(s.lines[0].date, Date { year: 2026, month: 8, day: 2 });
}

#[test]
fn european_statement_amounts() {
    let s = eu("01.08.2026;x;1.234,56;A\n02.08.2026;y;7,05;B\n");
    assert_eq!(s.lines.iter().map(|l| l.amount_cents).collect::<Vec<_>>(), vec![123_456, 705]);
}

#[test]
fn european_negative_amounts() {
    assert_eq!(eu("01.08.2026;x;-2.450,00;A\n").lines[0].amount_cents, -245_000);
}

#[test]
fn european_large_amounts() {
    assert_eq!(eu("01.08.2026;x;1.000.000,01;A\n").lines[0].amount_cents, 100_000_001);
}

#[test]
fn european_dates() {
    let s = eu("31.12.2025;x;1,00;A\n");
    assert_eq!(s.lines[0].date, Date { year: 2025, month: 12, day: 31 });
}

#[test]
fn unknown_statement_format_is_rejected() {
    assert!(matches!(parse_statement("bank_c_2026-08.csv", "when|what|how much\n1|2|3\n"), Err(ParseError::UnknownFormat(_))));
}

#[test]
fn blank_lines_are_ignored() {
    assert_eq!(eu("\n01.08.2026;x;1,00;A\n\n").lines.len(), 1);
    assert_eq!(us("\n2026-08-01,x,1.00,A\n\n").lines.len(), 1);
}

#[test]
fn ledger_parses() {
    let l = led("L-1\t2026-08-01\tops\tAcme\tINV-1\t-10.00\n");
    assert_eq!((l[0].account.as_str(), l[0].amount_cents), ("ops", -1000));
}

#[test]
fn ledger_bad_row_is_an_error() {
    assert!(matches!(parse_ledger(&format!("{LH}L-1\t2026-08-01\tops\n")), Err(ParseError::BadLine { .. })));
}

#[test]
fn exact_match() {
    let rec = reconcile(&[us("2026-08-01,x,5.00,INV-1\n")], &led("L-1\t2026-08-01\tp\tA\tINV-1\t5.00\n"));
    assert_eq!(rec.matched.len(), 1);
    assert_eq!(rec.matched[0].lines, vec![("bank_b_2026-08.csv".to_string(), 0)]);
}

#[test]
fn references_match_ignoring_case_and_padding() {
    let rec = reconcile(&[us("2026-08-01,x,5.00,  inv-1 \n")], &led("L-1\t2026-08-01\tp\tA\tINV-1\t5.00\n"));
    assert_eq!(rec.matched.len(), 1);
    assert!(rec.unmatched_bank.is_empty() && rec.unmatched_ledger.is_empty());
}

#[test]
fn split_payments_match_one_entry() {
    let rec = reconcile(&[us("2026-08-01,x,9.00,INV-7\n2026-08-09,x,6.00,inv-7\n")], &led("L-7\t2026-08-01\tp\tA\tINV-7\t15.00\n"));
    assert_eq!(rec.matched.len(), 1);
    assert_eq!(rec.matched[0].lines.len(), 2);
    assert!(rec.unmatched_bank.is_empty());
}

#[test]
fn split_payments_that_do_not_sum_stay_unmatched() {
    let rec = reconcile(&[us("2026-08-01,x,9.00,INV-7\n2026-08-09,x,5.00,INV-7\n")], &led("L-7\t2026-08-01\tp\tA\tINV-7\t15.00\n"));
    assert!(rec.matched.is_empty());
    assert_eq!(rec.unmatched_ledger, vec!["L-7".to_string()]);
    assert_eq!(rec.unmatched_bank.len(), 2);
}

#[test]
fn unmatched_ledger_entries_are_reported() {
    let rec = reconcile(&[us("2026-08-01,x,5.00,INV-1\n")], &led("L-1\t2026-08-01\tp\tA\tINV-1\t5.00\nL-2\t2026-08-02\tp\tB\tINV-2\t7.00\n"));
    assert_eq!(rec.unmatched_ledger, vec!["L-2".to_string()]);
}

#[test]
fn unmatched_bank_lines_are_reported() {
    let rec = reconcile(&[us("2026-08-01,x,5.00,INV-1\n2026-08-03,y,1.00,MISC\n")], &led("L-1\t2026-08-01\tp\tA\tINV-1\t5.00\n"));
    assert_eq!(rec.unmatched_bank.len(), 1);
    assert_eq!(rec.unmatched_bank[0].description, "y");
}

#[test]
fn each_bank_line_is_used_once() {
    let rec = reconcile(
        &[us("2026-08-01,x,5.00,INV-1\n2026-08-02,x,5.00,INV-1\n")],
        &led("L-1\t2026-08-01\tp\tA\tINV-1\t5.00\nL-2\t2026-08-02\tp\tA\tINV-1\t5.00\nL-3\t2026-08-03\tp\tA\tINV-1\t5.00\n"),
    );
    assert_eq!(rec.matched.len(), 2);
    assert_eq!(rec.unmatched_ledger, vec!["L-3".to_string()]);
}

const ACCOUNTS: &str = "statement_prefix,account\nbank_a,operating\nbank_b,payroll\n";

#[test]
fn variance_per_account() {
    let st = [eu("01.08.2026;x;100,00;A\n"), us("2026-08-01,x,50.00,B\n2026-08-02,y,-5.00,C\n")];
    let l = led("L-1\t2026-08-01\toperating\tX\tA\t100.00\nL-2\t2026-08-01\tpayroll\tY\tB\t70.00\n");
    assert_eq!(
        variance_report(&st, &l, ACCOUNTS, "2026-08").unwrap(),
        vec![
            AccountVariance { account: "operating".into(), ledger_cents: 10_000, bank_cents: 10_000, variance_cents: 0 },
            AccountVariance { account: "payroll".into(), ledger_cents: 7_000, bank_cents: 4_500, variance_cents: -2_500 },
        ]
    );
}

#[test]
fn variance_only_counts_the_month() {
    let st = [us("2026-07-31,x,1.00,A\n2026-08-01,x,2.00,B\n2026-09-01,x,4.00,C\n")];
    let l = led("L-1\t2026-07-31\tpayroll\tX\tA\t1.00\nL-2\t2026-08-15\tpayroll\tX\tB\t3.00\n");
    let r = variance_report(&st, &l, ACCOUNTS, "2026-08").unwrap();
    let payroll = r.iter().find(|a| a.account == "payroll").unwrap();
    assert_eq!((payroll.ledger_cents, payroll.bank_cents, payroll.variance_cents), (300, 200, -100));
}

#[test]
fn unmapped_statement_is_an_error() {
    let st = [parse_statement("bank_z_2026-08.csv", &format!("{US}2026-08-01,x,1.00,A\n")).unwrap()];
    assert_eq!(
        variance_report(&st, &[], ACCOUNTS, "2026-08"),
        Err(ReportError::UnmappedStatement("bank_z_2026-08.csv".into()))
    );
}

#[test]
fn every_account_is_listed_sorted() {
    let accounts = "statement_prefix,account\nbank_b,zeta\nbank_a,alpha\nbank_c,mid\n";
    let r = variance_report(&[], &[], accounts, "2026-08").unwrap();
    assert_eq!(r.iter().map(|a| a.account.as_str()).collect::<Vec<_>>(), vec!["alpha", "mid", "zeta"]);
    assert!(r.iter().all(|a| a.variance_cents == 0));
}
