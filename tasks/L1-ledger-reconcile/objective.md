The August month-end reconciliation (`cargo run -- data 2026-08`) reports hundreds of false
discrepancies. `logs/reconcile.log` has the output of the failed close; the inputs are in
`data/`.

1. Find and fix every cause of the false discrepancies described in the log. The real statement
   files in `data/statements/` and the ledger in `data/ledger.tsv` are authoritative for formats
   and conventions.
2. Implement `report::variance_report`, which is currently a stub: for the given month
   (`YYYY-MM`), for every account listed in `data/accounts.csv`, compute the ledger total, the
   bank total (all statement lines of files mapped to that account) and
   `variance = bank - ledger`, in cents. Return one row per account, sorted by account name,
   including accounts with zero variance. A statement file whose prefix is not in the accounts
   file is an error (`ReportError::UnmappedStatement(file_name)`).
3. Keep every public signature in `src/` unchanged, and keep the existing tests passing.

Do not copy data from `data/` or the log into source or tests; write synthetic fixtures. Use
only the standard library.
