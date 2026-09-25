# Statement fingerprints

The query gateway writes one audit record per statement. Audit records must not contain customer
data, so from the 2026-10 release the gateway logs a *fingerprint* of each statement instead of its
text. A fingerprint keeps the shape of a statement (keywords, identifiers, operators) and replaces
every constant with `?`, so two runs of the same report with different customers produce the same
fingerprint.

## API

```rust
// src/fingerprint.rs, exported as `sqlparser::fingerprint`
pub fn fingerprint(dialect: &dyn Dialect, sql: &str) -> Result<String, TokenizerError>;
```

Fingerprinting works on tokens, not on the syntax tree: the gateway also fingerprints statements
that it rejects, so a statement that does not parse must still get a fingerprint. Only a
tokenizer error returns `Err`.

## Rules

1. **Tokens.** `sql` is split into tokens by this crate's tokenizer for `dialect` (the same
   tokenizer the parser uses).
2. **Whitespace and comments** are dropped. They only separate tokens.
3. **Constants** become `?`: every string constant the tokenizer produces (single-quoted,
   double-quoted when the dialect treats double quotes as strings, `N'…'`, `E'…'`, `X'…'`, byte,
   raw, triple-quoted and dollar-quoted strings) and every number. A string constant continued over
   several lines (PostgreSQL: string constants separated only by whitespace that contains a newline
   are one constant) is one constant, so it becomes a single `?`.
4. **Signs.** A `-` or `+` directly before a number is part of the constant (`-5` becomes `?`) when
   the token before the sign is absent, is an operator or punctuation other than `)` and `]`, or is
   one of the keywords `SELECT`, `WHERE`, `AND`, `OR`, `NOT`, `ON`, `HAVING`, `WHEN`, `THEN`,
   `ELSE`, `BETWEEN`, `LIMIT`, `OFFSET`, `RETURNING`. After anything else (an identifier, another
   keyword, a constant, a placeholder, `)` or `]`) the sign is a binary operator and is kept:
   `a - 1` is `a - ?`, `x = -1` is `x = ?`.
5. **Words.** An unquoted word that is a keyword of this crate (`src/keywords.rs`) is written in
   upper case; any other unquoted word is written in lower case. Quoted identifiers are written
   as they appear, with their quotes.
6. **Placeholders** (`$1`, `?` in dialects that use it, `:name`) are kept as they are; they are not
   constants.
7. **Lists of constants** collapse:
   - `IN (?, ?, …)` becomes `IN (?)` when every element is a single constant;
   - `ARRAY[?, ?, …]` becomes `ARRAY[?]` (rendered `ARRAY [?]`, see rule 8) when every element is a
     single constant;
   - in the rows of a `VALUES` list, a row that renders exactly like the row before it is dropped,
     together with its comma: `VALUES (?, ?), (?, ?)` becomes `VALUES (?, ?)`, while
     `VALUES (?, ?), (?, DEFAULT), (?, ?)` keeps all three rows.
   Lists that contain anything else (identifiers, placeholders, expressions) are kept.
8. **Rendering.** Tokens are written with their SQL text and separated by one space, except that
   there is no space after `(`, `[`, `.` and `::`, and none before `)`, `]`, `,`, `.` and `::`.
9. **Statements.** `;` separates statements. Empty statements are dropped; the fingerprints of the
   remaining statements are joined with `; ` (no trailing `;`).

## Examples (PostgreSQL dialect)

| Statement | Fingerprint |
|---|---|
| `select Name from Customers where email = 'x@example.com'` | `SELECT NAME FROM customers WHERE email = ?` |
| `SELECT count(*) FROM orders WHERE id IN (1, 2, 3) -- ticket 7` | `SELECT COUNT (*) FROM orders WHERE id IN (?)` |
| `SELECT a - 1, -1 FROM t LIMIT 10` | `SELECT a - ?, ? FROM t LIMIT ?` |
| `SELECT "Kunde"."Name" FROM "Kunde" WHERE id = $1` | `SELECT "Kunde"."Name" FROM "Kunde" WHERE id = $1` |
| `INSERT INTO t (a, b) VALUES (1, 'x'), (2, 'y')` | `INSERT INTO t (a, b) VALUES (?, ?)` |
| `SELECT x::text FROM t; SELECT 1;` | `SELECT x::TEXT FROM t; SELECT ?` |
