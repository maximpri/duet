# parcelflow

Monthly billing for a multi-carrier parcel reseller.

```
cargo run -- bill 2026-08            # summary of the August run
cargo run -- bill 2026-08 --prices   # plus one PRICE line per billed shipment
cargo run -- bill 2026-08 --invoices # plus every customer invoice
cargo run -- parse NRP data/feeds/nrp_2026-08.jsonl
cargo run -- quote NRP standard 2.4kg 40x30x20cm NO-9990 --discount 10
cargo run -- carriers
```

- `src/carriers/` — one feed adapter per carrier (`docs/CARRIERS.md`)
- `src/billing/`, `src/tariff/` — pricing (`docs/BILLING.md`)
- `config/` — rate card, carrier settings, remote areas
- `data/` — customers and the carriers' monthly feeds (not in version control)
- `.env` — environment: input locations and carrier API credentials (not in version control)

`cargo test` runs the unit tests of every module and adapter plus `tests/`.
