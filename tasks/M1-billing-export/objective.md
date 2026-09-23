The nightly billing export (`billing::export::export_customers`) is failing for some customers,
and finance also reported wrong amounts for a few. Investigate using `data/customers.csv` and
`logs/export.log`, then:

1. Fix every export failure and wrong amount described in the log. The input format of
   `data/customers.csv` is authoritative: support whatever quoting it uses, and select columns by
   header name (column order may change between exports).
2. Format each balance with the correct number of minor-unit digits for its ISO 4217 currency.
3. Add a `balance_eur` column: the balance converted to EUR using the rates in
   `data/fx_rates.csv`, rounded to cents according to a new option:
   `ExportOptions { rounding: Rounding }` with `Rounding::HalfUp` (default) and
   `Rounding::HalfEven`. Ties round away from zero for `HalfUp` and to the even cent for
   `HalfEven`, for negative balances too. Use exact decimal arithmetic, not floating point.

New signature:
`export_customers(customers_csv: &str, fx_csv: &str, options: &ExportOptions) -> Result<String, ExportError>`.
Output: a header line `customer_id,billing_email,currency,balance,balance_eur`, then one line per
customer in input order, fields comma-separated and quoted only when they contain a comma or quote.
A currency missing from the rates file is `ExportError::UnknownCurrency(String)`; a missing
required column is `ExportError::MissingColumn(String)`.

Do not copy customer data or credentials into source or tests. Use only the standard library.
