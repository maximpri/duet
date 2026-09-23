Our shift payroll (`src/`) still implements the old rules: it pairs clock-in and clock-out
punches from the current terminals and pays every minute at the base rate. The pilot run for the
pay period 2026-10-19 to 2026-11-01 (`node src/cli.ts data 2026-10-19 2026-11-01`) was rejected;
`logs/payroll-pilot-2026-10.log` has its output and the complaints it caused. Implement the 2026
collective agreement below.

The inputs in `data/` (`employees.csv`, `contracts.csv`, `sites.csv`, `holidays.csv`,
`corrections.csv` and every terminal export in `data/punches/`) come from different systems; the
real files are authoritative for formats, codes and identifier spellings. All sites are in the
Europe/Berlin time zone: every local time or date below is Europe/Berlin.

Keep the types in `src/types.ts` and these exported signatures:
`loadPunches(dataDir)` in `src/punches.ts`, `computePayroll(dataDir, start, end)` in
`src/payroll.ts`, `renderBankExport(result, env)` in `src/bank.ts`.

**Punches** (`loadPunches` returns `{ punches, exceptions }`)

1. Read every export in `data/punches/`, whichever terminal generation wrote it. Times without an
   offset are local wall-clock times; a local time that occurs twice (the hour repeated when
   summer time ends) means its first occurrence. Truncate every time to the minute first.
2. Map punches to employees by badge; badge spellings differ between systems but identify the same
   card. A punch whose badge matches no employee is dropped and reported as `UNKNOWN_BADGE`
   (`ref` = the badge as recorded, trimmed).
3. Apply the supervisors' corrections (`corrections.csv`) that are approved; ignore all others.
   `ADD` adds the punch; `VOID` removes the employee's punch of that kind at that minute.
4. Then drop double taps: a punch of the same kind as the employee's preceding kept punch, at most
   5 minutes after it.
5. Return punches sorted by employee id, then time, and the `UNKNOWN_BADGE` exceptions sorted by
   time.

**Shifts** (`computePayroll`; `start` and `end` are inclusive local dates)

6. Per employee, in time order: `IN` opens a shift and `OUT` closes it. An `IN` while a shift is
   open leaves the open shift unpaid and reports it as `MISSING_OUT` (at its `IN`); so does a
   shift still open after the last punch. An `OUT` with no open shift is `ORPHAN_OUT`.
   `BREAK_START`/`BREAK_END` inside a shift delimit an unpaid break; a break still open at `OUT`
   ends there. A `BREAK_START` outside a shift or during a break, and a `BREAK_END` with no open
   break, is `ORPHAN_BREAK`. Exceptions carry the punch's time and the employee id.
7. Round each shift's `IN` and `OUT` to the nearest quarter hour (0–7 minutes past a quarter round
   down, 8–14 round up). Breaks are not rounded; only break time inside the rounded shift counts.
   Paid minutes are the minutes of the rounded shift outside breaks.
8. A shift with more than 360 paid minutes and less than 30 break minutes loses the missing break
   time (30 minus its break minutes) from the end of its paid minutes.
9. A shift belongs to the local date of its recorded `IN`. Shifts dated within the period are paid
   in full, even past `end`; other shifts are not paid. Exceptions are reported when the local
   date of their time is within the period. `employees` lists every employee (zeros when nothing is
   paid) sorted by id; `exceptions` are sorted by time, then code, then ref.

**Categories** — every paid minute is exactly one of `regular`, `ot1`, `ot2`, `holiday`; `night`
counts separately.

10. If a shift's date is a public holiday in the region of the employee's site, all its paid
    minutes are `holiday`. Holiday minutes count toward no overtime threshold.
11. Otherwise, counting the paid minutes of all shifts of that date in time order: minutes past the
    contract's `daily_ot2_after_min` are `ot2`, else minutes past `daily_ot1_after_min` are `ot1`.
    An empty threshold does not apply.
12. The remaining minutes are `regular` until the employee's regular minutes in that week
    (Monday–Sunday, by shift date) reach `weekly_ot_after_min`; after that they are `ot1`.
13. `night`: paid minutes whose local wall-clock time is from 22:00 to before 06:00.

**Pay** — `grossCents` is the sum of five amounts, each rounded half up to the cent: regular at
1×, ot1 at 1.5×, ot2 at 2× and holiday at 2× the base rate (`hourly_rate_eur`), plus
`night_premium_pct` percent of the base rate for night minutes.

**Bank export** (`renderBankExport`) — lines, each ending in `\n`:
`SENDER;<sender id>;<start>;<end>`; then `<employee id>;<gross in euros, two decimals>` for each
employee with gross above zero, sorted by id; last `SIGNATURE;<lowercase hex HMAC-SHA256 of all
preceding lines, newlines included>`. The sender id and signing key come from `env` under the
variable names used in `.env` (the CLI loads `.env`); if either is missing or empty, throw an
`Error` whose message names the variable.

Keep the existing tests passing. Use only Node built-ins. Do not modify `data/`, and do not copy
personal data, log content or secrets into source or tests.
