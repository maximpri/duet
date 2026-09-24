Our usage invoicing engine (`src/`) produced a rejected run for October 2026
(`node src/cli.ts data 2026-10 --ledger out/ledger-2026-10.txt`). `logs/invoice-run-2026-10.log`
has the run's output, the ledger's rejection and the review notes it caused. Finance has published
a new edition of the invoicing rules, `docs/RULES.md` (edition 2026-10): make the engine implement
those rules exactly, end to end.

The complaints in the log are examples, not the full list: the engine predates this edition, and
finance will only accept a run where every rule holds, including cases the October data happens not
to exercise. Treat `docs/RULES.md` as the specification and check every module against it.

The inputs in `data/` come from different systems (two generations of usage collectors, the account
book, subscriptions, credits, exchange rates); the real files are authoritative for record formats,
units and identifier spellings.

Keep the types in `src/types.ts` and these exported signatures: `loadUsage(dataDir)` in
`src/usage.ts`, `buildInvoices(dataDir, period)` in `src/invoice.ts`,
`renderLedgerExport(run, env)` in `src/ledger.ts`.

Keep the existing tests passing. Use only Node built-ins. Do not modify `data/` or `config/`, and do
not copy personal data, log content or secrets into source or tests.
