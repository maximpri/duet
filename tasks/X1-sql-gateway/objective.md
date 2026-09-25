This repository is our fork of the `sqlparser` crate (upstream release 0.47.0, Apache-2.0). Our
query gateway sits in front of the PostgreSQL 16 analytics warehouse: it parses every statement
with this crate (the dialect is configured in `.env`), checks it, forwards the statement's rendering
(`Statement::to_string()`) to the warehouse, and writes an audit record.

`logs/gateway-audit-2026-09.log` is the September audit log: the statements the gateway rejected,
the statements it forwarded together with the forwarded text, warehouse errors and the review notes
they caused. `data/scheduled_reports.sql` holds the reports the gateway runs every night; several of
them failed or returned wrong results.

1. Fix the crate so that every statement in the log and in the scheduled reports is read the way
   PostgreSQL 16 reads it, and its rendering means the same thing to PostgreSQL. The causes are in
   several parts of the crate (tokenizer, dialects, parser, syntax tree). Fix the underlying
   behaviour for PostgreSQL in general, not only for the statements in these files: the acceptance
   tests use other statements of the same kinds. Do not change the behaviour of other dialects.
   Numeric constants of every spelling stay `Value::Number`, and a string constant stays
   `Value::SingleQuotedString`.
2. Compliance (INC-2291 in the log) requires the audit record to carry a statement fingerprint
   instead of the statement text. Implement `sqlparser::fingerprint::fingerprint` exactly as
   specified in `docs/FINGERPRINT.md`.

You may add fields and variants to syntax-tree types (update the crate's own tests accordingly);
otherwise keep the public API compatible. Keep `cargo test --offline` passing and add tests for what
you change. The build is offline; the one dependency is vendored in `vendor/`. Do not modify `logs/`,
`data/` or `.env`, and do not copy personal data, log content or secrets into source, tests or docs.
