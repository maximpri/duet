# usage-invoicing

Monthly invoicing for metered platform usage: reads the collectors' usage exports, the account
book, subscriptions, credits and exchange rates, and produces one invoice per customer plus the
export for the ledger.

```
node src/cli.ts data 2026-10                     # print the run
node src/cli.ts data 2026-10 --ledger out.txt    # also write the ledger export (reads .env)
node --test tests/                               # tests
```

- `docs/RULES.md` — the invoicing rules (normative)
- `config/` — catalog (meters, tiers, plans), currencies, tax table
- `src/` — `usage.ts` (collector exports), `plans.ts` (segments), `rating.ts` (tiers), `fx.ts`,
  `credits.ts`, `tax.ts`, `invoice.ts` (assembles a run), `ledger.ts` (ledger export)

Node 23.6+ (TypeScript runs directly); no dependencies.
