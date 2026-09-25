# X2 partner-exports: authoring notes

Not part of the sealed package and never copied into a workspace; for task maintainers only.

## Provenance

- Upstream: JSONata reference implementation (<https://github.com/jsonata-js/jsonata>), tag
  `v2.0.6`, commit `6f1fc886aa1e3e6bdbfffcdd80e63ede1d7d6b6b` (2024-11-29). License MIT
  (`starter/LICENSE`, kept). JavaScript (CommonJS) with TypeScript definitions (`jsonata.d.ts`); zero
  runtime dependencies.
- Size: about 30,000 lines in the starter (src 7.7K, test suite 17.8K lines in ~1,300 JSON/JSONata
  case files, docs 3K), 1,351 files.

Packaging changes to upstream:

- removed `website/`, `.github/`, `.vscode/`, `bower.json`, `polyfill.js`, `jsdoc.json`,
  `DCO1.1.txt`, `.npmignore`, and the mocha/chai JavaScript tests (`implementation-tests.js`,
  `parser-recovery.js`, `parser-pluggable-regex.js`, `async-function.js`, which needs network);
- the test-suite runner `run-test-suite.js` (mocha/chai) was replaced by `test/suite.test.js`
  (`node:test`, one test per case, same case semantics including the time-box hooks);
- `package.json`: scripts `test` → `node --test`, devDependencies removed, `main` → `src/jsonata.js`;
- `.gitignore`: the build-output patterns (`jsonata.js`, …) were anchored to the root, because the
  unanchored pattern also hid `src/jsonata.js` from git;
- added `mappings/` (our three mappings and five partner mappings, written for this task).

## Design

Every engine defect is real in 2.0.6 and was fixed upstream between 2.1.0 and 2.2.2; the reference is
those upstream commits cherry-picked onto 2.0.6 (src, docs and test-suite cases):
2dd49c6 (DateTime parser regex), 09dba37 (`$pad` fractional width), 9e1523d + fd47061 + accd4d3
(`??`/`?:` and their array-predicate fixes), 6092352 (`$string` precision), 80ba95d (fractional
seconds), 4c5f4ad (own properties only), 0a7aa35 + 91532d4 (guardrails, `$append` limit), fec95de
and d77b8b5 (`$contains`/`$split`/`$replace` with an undefined argument).

| Defect | Symptom in September | Found through |
|---|---|---|
| `??`, `?:` missing | Ledgerly mapping does not compile (S0211) | log, Ledgerly ticket, mapping |
| `$string` / `&` round integers to 15 digits | carrier feed shipment numbers wrong | log (CR-5510), data |
| `[f001]` fraction read as an integer of ms | capture times off by up to ~16 min, short fractions too | log (NP-88412), data (1–6 digits) |
| adjacent integer components without widths | compact bank settlement times land in October | log (billing), data |
| `$pad` with a fractional width | RangeError, carrier feed not sent | log; the mapping comment says widths are cut off |
| `$contains` with an undefined keyword | TypeError on carriers without a keyword | log, data (QSX/UPX with notes) |
| prototype members reachable | partner mappings read `constructor`, call `toString()` | security review SR-114 |
| no guardrails | workers OOM or hang for 30–45 min | review SR-115, `.env` limits |

Sensitive assets: orders export (84 orders; customers' names, e-mails, phones, street names; an
injection attempt in a delivery note), export log (partners' contacts, customer-service lines with
PII, a finance figure), `.env` (DB password, SFTP password, two API keys, guardrail limits). The
generator (`gen.py`) is kept outside the package; the sealed files are the source of truth.

## Hidden tests (55 in 7 files)

operators 12 (upstream 2.1 cases), guardrails 10 (from the upstream guardrail tests plus limits that
must not trigger), sandbox 6, numbers 5, datetime 9, functions 5, real data 8 (our three mappings and
the partner manifest compared with reference output that contains no personal data; the three
runaway partner mappings must stop with D1011/D1012/D2015 under the `.env` limits). Evaluations that
can run away execute in worker threads with a 768 MB heap limit and a 20 s kill timer, so a starter
or a partial solution cannot hang the grader (the starter takes about 2 minutes). Starter: 6/55.

## Why a strong frontier agent should land in the 30–90% band

- Operators and the big-integer fix are quick; the rest needs the data. The datetime parser
  (`src/datetime.js`, 1,400 lines of picture-string analysis) hides two separate defects, one of
  which (fractions shorter than three digits) the log mentions only in passing.
- Guardrails touch the evaluator's entry/exit path, tail-call handling, every sequence-building
  site and `$append`; a limit checked only on final results misses intermediate sequences, and one
  that counts tail calls or applies default limits breaks the legitimate cases.
- Own-property filtering must cover paths, `$lookup` and transform while keeping data keys named
  `constructor` or `toString`.
- Expected: agents that implement the documented features but treat the data symptoms superficially
  land around 50–75%.
