# Invoicing rules (edition 2026-10)

These rules are normative: the invoicing engine in `src/` must implement them exactly. Finance
signs off a run only when every invoice follows them. Configuration lives in `config/`; the monthly
inputs live in the data directory passed to the engine (`data/` in production).

Throughout, "exact" means no binary floating point: quantities, prices, rates and amounts are
decimal (or rational) values, and the only rounding steps are the ones named in §6, §7 and §8.

## 1. Accounts

`accounts.csv` has one row per account: `account_id`, `parent_id`, `company`, `contact_name`,
`contact_email`, `contact_phone`, `country`, `vat_id`, `currency`, `timezone`,
`contract_value_eur`.

1.1 An account with an empty `parent_id` is a **root**. Every other account is billed on the
invoice of its root, found by following `parent_id` upwards (any depth).

1.2 The root's `timezone`, `currency`, `country` and `vat_id` apply to everything on its invoice,
including the usage of its descendants. Descendants' own values for these columns are ignored.

1.3 `account_id` in `accounts.csv` is the canonical spelling used in all output.

## 2. Usage events

2.1 Every file in the `usage/` subdirectory of the data directory is a collector export: `*.csv`
files come from the edge-a collectors, `*.jsonl` files from the edge-b collectors. Each record
carries an event id, an account, a meter code, a quantity (with its unit) and an instant.

2.2 Units. Quantities are converted exactly into the meter's canonical unit (`config/catalog.json`):

| Canonical unit | Accepted units |
|---|---|
| `request` | `request` |
| `GiB-h` | `GiB-h`; `MiB-h` (1 MiB-h = 1/1024 GiB-h) |
| `GB` | `GB`; `MB` (1 MB = 1/1000 GB); `B` (1 B = 1/10^9 GB) |
| `min` | `min` |

A record without a unit is in the meter's canonical unit.

2.3 An event id identifies one event across all collectors: during a failover one collector may
deliver another collector's events again. Each event is counted once.

2.4 A record whose account matches no account in `accounts.csv` is not billed and is reported as
an `UNKNOWN_ACCOUNT` exception (`ref` = the account as recorded, trimmed). A record of a known
account whose meter is not in the catalog is reported as `UNKNOWN_METER` (`ref` = the meter code
as recorded). `count` is the number of events (after 2.3) with that code and ref. Exceptions cover
every record in the files, whatever its time.

2.5 An event belongs to billing period `YYYY-MM` of its root account if its instant is at or after
local midnight at the start of the first day of that month and before local midnight at the start
of the first day of the next month, in the root's time zone.

## 3. Plans and segments

`subscriptions.csv`: `account_id` (a root), `plan` (a plan code from the catalog, or `none` for no
plan), `effective_from` (local date and time `YYYY-MM-DD HH:MM` in the root's time zone).

3.1 The plan in force at an instant is the one of the row with the latest `effective_from` at or
before that instant; before the first row there is no plan.

3.2 The period is split into **segments** at each instant strictly inside it where the plan in
force changes to a different code (a row that repeats the plan in force does not split). A
segment's length is the real elapsed time in seconds between its bounds (instants, not wall-clock
hours: a day may have 23 or 25 hours). The period's length is measured the same way.

3.3 Segments without a plan (`none` or before the first row) are not billed, and neither is the
usage that falls in them. A root account with no billed segment in the period gets no invoice.

## 4. Fees

4.1 Each billed segment has one `fee` line: the plan's `monthly_fee_eur` multiplied by
segment length / period length (exact). Its `quantity` is the segment length in seconds.

## 5. Usage

5.1 For each billed segment and meter, the **raw quantity** is the sum of the events (of the root
and all its descendants) in the segment, in the canonical unit.

5.2 The segment's **included quantity** of a meter is the plan's `included` quantity for that
meter multiplied by segment length / period length, rounded down to a whole unit (zero if the plan
includes none). The **billable quantity** is the raw quantity minus the included quantity, or
zero if that is negative.

5.3 Tiers (`config/catalog.json`) are measured on the billable quantity of the segment; they start
from zero in every segment. A tier covers quantities above the previous tier's `up_to` up to and
including its own `up_to`; `null` means unbounded.

- `graduated`: each part of the billable quantity is priced at the unit price of the tier it
  falls in, and the parts are added.
- `volume`: the whole billable quantity is priced at the unit price of the tier that contains it.

5.4 A `usage` line is produced for each segment and meter with a billable quantity above zero.
Its `quantity` is the billable quantity.

## 6. Currency and rounding

6.1 Line amounts are exact in EUR until they are converted: the amount of a line in the invoice
currency is its exact EUR amount multiplied by the exchange rate, rounded once, half to even, to
the currency's minor unit (`config/currencies.json`: number of decimals).

6.2 `fx.csv` rows (`date`, `currency`, `rate`) mean 1 EUR = `rate` units of `currency`. An
invoice uses the rate of its currency with the latest `date` on or before the last day of the
period. EUR has rate 1. A missing rate is an error.

## 7. Credits

`credits.csv`: `credit_id`, `account_id`, `kind`, `granted_on`, `expires_on` (last valid day,
inclusive; empty = never expires), `remaining_eur`, `note`.

7.1 A credit is eligible for a period's invoice if its account is the root or one of its
descendants, `granted_on` is on or before the last day of the period, `expires_on` is empty or on
or after the first day of the period, and `remaining_eur` is above zero.

7.2 A credit's value is `remaining_eur` converted to the invoice currency with the invoice's rate,
rounded half to even to the minor unit.

7.3 Credits apply only to usage: the sum of the invoice's usage line amounts. Eligible credits are
applied in this order: earliest `expires_on` first (credits that never expire last), then earliest
`granted_on`, then `credit_id`. Each applies its value or what is left of the usage sum, whichever
is smaller. Only credits that apply an amount above zero are listed.

## 8. Tax

8.1 The seller is established in `DE`. For the root's `country` (`config/tax.json`):

- `DE`: rule `domestic`, the `DE` rate;
- another EU member with a non-empty `vat_id`: rule `reverse_charge`, rate 0;
- another EU member without a `vat_id`: rule `oss`, that country's rate;
- anywhere else: rule `export`, rate 0.

8.2 Tax is computed once per invoice: (subtotal − credit total) × rate / 100, rounded half to even
to the minor unit. The invoice total is subtotal − credit total + tax.

## 9. Output

9.1 One invoice per root account with a billed segment, sorted by `account_id` (plain string
order). Lines are ordered by segment; within a segment the fee line comes first, then usage lines
by meter code.

9.2 Quantities are written as plain decimals: no exponent, no trailing zeros after the decimal
point, no trailing decimal point.

9.3 Exceptions are sorted by code, then ref.

## 10. Ledger export

The ledger export for a run is text; every line ends with `\n`:

1. `BATCH;<sender id>;<period>;<number of invoices>`
2. for each invoice, in invoice order: `INV;<account_id>;<currency>;<subtotal>;<credit total>;<tax>;<total>`,
   amounts in major units with exactly the currency's number of decimals and `.` as the separator;
3. `SIG;<lowercase hex HMAC-SHA256 of all preceding lines, newlines included>`.

The sender id and the signing key come from the environment of the production deployment, under
the variable names used in its `.env`. If either is missing or empty, the export fails with an
error naming the variable.
