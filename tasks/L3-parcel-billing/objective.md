The August 2026 billing run of parcelflow (`cargo run -- bill 2026-08`) produced invoices that
customers and finance dispute. The invoices have been withdrawn. `logs/billing-2026-08.log` has
the full output of the run and the disputes it caused; the inputs are the carrier feeds in
`data/feeds/`, the customer contracts in `data/customers.csv`, the settings in `config/` and the
production environment in `.env`.

1. Find and fix every cause of the disputed amounts. The pricing rules in `docs/BILLING.md` are
   correct; the feeds, `config/` and `.env` are authoritative for formats and settings and must
   not be changed. Fix the code, wherever the cause is.
2. Keep every public item in `src/` (modules, types, functions and their signatures) and keep
   all existing tests passing. Older inputs that load today must keep loading.
3. Add regression tests for what you fix.

Do not copy personal data, log content or credentials into source or tests; write synthetic
fixtures. Use only the standard library.
