use ledger::entries::parse_ledger;
use ledger::reconcile::reconcile;
use ledger::statement::parse_statement;

#[test]
fn exact_match_reconciles() {
    let s = parse_statement("bank_b_2026-08.csv", "date,description,amount,reference\n2026-08-02,Fee,-12.50,FEE-1\n").unwrap();
    let l = parse_ledger("entry_id\tdate\taccount\tcounterparty\treference\tamount\nL-1\t2026-08-02\tpayroll\tBank\tFEE-1\t-12.50\n").unwrap();
    let rec = reconcile(&[s], &l);
    assert_eq!(rec.matched.len(), 1);
    assert!(rec.unmatched_bank.is_empty() && rec.unmatched_ledger.is_empty());
}
