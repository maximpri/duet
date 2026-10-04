# X1 follow-up: grader correction and retry cost, 2026-09-30

The next steps in [the hand-off](https://github.com/maximpri/duet/blob/9b0ac104ba6947e338ca3baacfd31de2c2823e83/docs/HANDOFF-2026-09-29.md) were to trace X1's ANY/ALL failures and
check whether the chunk retry raised its cost. The five-test cluster is a grader defect. Correcting
it gives default hybrid 49/49/48 of 50 (97.3%) and passthrough 48/49 (97.0%) on the saved artifacts.
No frontier prompt, privacy rule or agent-generated solution was changed.

## What the failing runs actually saw

The hand-off's claim that every seed failed the cluster was wrong: `x1-chunks` seed 1 passed all
five original tests, as did several other runs. Seeds 2 and 3 saw the necessary constructs:

- Seed 2's local answers described `ANY (subquery)`, row ANY, ALL and ALL with a CTE. The frontier
  implemented parsing, inspected the double-parenthesis rendering and explicitly accepted it.
- Seed 3 received a generic keyword/punctuation example for the rejected ANY query, and structural
  report descriptions containing row ANY and ALL with a CTE. Its tests expected double parentheses.
- Both implementations retained `Expr::Subquery` as the right operand and the existing display
  code added another pair of parentheses. The five hidden failures were exactly that spelling
  difference, not a missing construct or missing evidence.

Evidence: each source run's `workspace/.duet/runs/*/transcript.jsonl`, `workspace/src/{parser,ast}/mod.rs`,
and `grade/hidden/test-output-{2,5}.txt`. Diagnostic extracts are in
`/Volumes/EXT_DISK/duet_v2/tmp-gate/x1-s{1,2,3}-qa.txt`.

## Independent PostgreSQL check

The original predicate helper asserted that `ANY ((SELECT …))` meant a scalar subquery used as
an array. PostgreSQL's grammar says otherwise: `select_with_parens` recursively accepts wrapper
parentheses, and the quantified-subquery production consumes it. The array-expression production
is separate. See [PostgreSQL 16's grammar](https://github.com/postgres/postgres/blob/REL_16_STABLE/src/backend/parser/gram.y)
and its [subquery comparison documentation](https://www.postgresql.org/docs/16/functions-subquery.html).

Built PostgreSQL **16.13** from the official source archive on the external disk and started an
isolated cluster listening only on its private Unix socket. The verification script checked:

- scalar ANY against multiple rows, with and without the extra parentheses;
- ALL with a CTE and row ANY against multiple rows;
- identical `pg_get_viewdef` results for the three pairs, as well as identical query results;
- NULL and empty-result behavior;
- the distinct array-expression case: a cast after the inner query is retained, while an
  unadorned array-valued subquery still produces the expected type error.

All assertions passed. PostgreSQL 18.4 also passed the initial three-pair smoke check; the
16.13 run is the version-relevant evidence. Script and output:
`results/x1-grader-v2/postgres-equivalence.sql` and `postgres16-proof.txt`.

## Grader revision 2

`holdout/tests/support/quantified.rs` tokenizes renderings/fingerprints and removes an inner pair
only when it wraps the entire quantified SELECT/WITH/VALUES query. It preserves casts,
operators, arrays, set-operation grouping, quantifiers and quoted text. Positive and negative
self-checks run within the existing predicate test; they add no points to the denominator.

The three predicate checks accept those equivalent spellings. The two forwarded-meaning checks
compare fingerprints after this normalization. The fingerprint API's exact-output tests and
original-statement fingerprint baselines remain strict. Starter, assets, objective, reference
solution and all previously generated solutions are unchanged.

`task.toml` identifies revision 2 and `seal.toml` includes the new helper and changed tests.
As required by the suite's version rule, earlier comparisons using revision 1 are superseded;
scores from the two seals must not be mixed. `duet-eval check X1` reports starter **3/50**,
reference **50/50**, all **779** reference visible tests passing, and no sink violations.

## Regraded artifacts

All 20 X1 artifacts in the four hand-off batches were graded with the new seal, using the existing
`duet-eval grade` command. This launches no model requests. Results are separate from the original
runs in `/Volumes/EXT_DISK/duet_v2/results/x1-grader-v2/`, with the seal, grading-binary hash,
grader source snapshot, original-record hashes, per-artifact JSON, and `summary.json`.
Every visible suite passed; there were no sink violations
or grading timeouts. Five artifacts gain five tests each; no artifact loses a passing test.

| Source batch | Lane | Seed | Revision 1 → 2, of 50 |
|---|---|---|---|
| m52-xl | hybrid + compaction | 1 | 48 → 48 |
| m52-xl | hybrid + compaction | 2 | 44 → 49 |
| m52-xl | hybrid + explorer | 1 | 50 → 50 |
| m52-xl | hybrid + explorer | 2 | 43 → 43 |
| m52-xl | hybrid | 1 | 42 → 42 |
| m52-xl | hybrid | 2 | 48 → 48 |
| m52-xl | passthrough | 1 | 48 → 48 |
| m52-xl | passthrough | 2 | 44 → 49 |
| x1-evidence | hybrid | 1 | 48 → 48 |
| x1-evidence | hybrid | 2 | 40 → 40 |
| x1-evidence | hybrid | 3 | 44 → 49 |
| x1-chunks | hybrid | 1 | 49 → 49 |
| x1-chunks | hybrid | 2 | 44 → 49 |
| x1-chunks | hybrid | 3 | 43 → 48 |
| x1-lanes | hybrid + compaction | 1 | 49 → 49 |
| x1-lanes | hybrid + compaction | 2 | 47 → 47 |
| x1-lanes | hybrid + compaction | 3 | 21 → 21 |
| x1-lanes | hybrid + explorer | 1 | 48 → 48 |
| x1-lanes | hybrid + explorer | 2 | 49 → 49 |
| x1-lanes | hybrid + explorer | 3 | 45 → 45 |

The compaction seed that ended at 21/50 still cannot compile all hidden binaries; 21 passed and
7 failed tests were actually reported, with the remaining 22 unreported. The denominator stays 50.
The old `x1-evidence` seed 2 and `x1-lanes` explorer seed 3 retain real parsing failures: correcting
the comparison does not excuse them.

The 97.3% vs 97.0% comparison is descriptive, across the existing builds and three vs two seeds;
it is not a new paired quality gate. Costs and original leak measurements are unchanged.
Compaction and the explorer stay off: the X2 failures, compaction collapse and limited explorer
use still hold.

## What the retry-cost records can show

The old records contain aggregate local requests, tokens and busy time, and only the final
sanitized answer to each question. They do not record the first chunk's failed answer, which
chunk was attempted, or malformed-output retries. Evidence lines in a final answer do not prove
whether a second part was tried. An exact retrospective retry count is therefore unavailable.

Three-seed means, from the original `run.json` files:

| Measure | Before chunk fix (`x1-evidence`) | After (`x1-chunks`) |
|---|---|---|
| Frontier requests | 144.3 | 180.3 |
| Frontier dollars | $0.4372 | $0.6078 |
| Total dollars | $0.4619 | $0.6387 |
| Local model calls | 39.3 | 38.3 |
| Local busy seconds | 1,783.3 | 1,446.0 |
| Wall seconds | 4,081.2 | 5,095.4 |

About $0.171 of the $0.177 increase is frontier spend. Local busy time fell by 337 seconds, so
extra local inference time does not explain the aggregate wall/cost increase. The runs took
different paths, however; these figures cannot isolate any indirect effect of a retry on later
frontier decisions. No new expensive batch is needed to establish the grader correction.

`CallStats` now records `answer_retries`, `answer_retry_seconds` and `malformed_retries`; the
evaluation ledger preserves them as `local_*` fields. Older evaluation records omit those fields
instead of presenting unknown counts as zero. Mock-provider tests verify the two retry kinds,
reset behavior and old-record compatibility. This adds telemetry without changing the prompts,
chunk ranking, retry limits or filtering.

Verification: the full `tools/gate.sh` passed (format, clippy, workspace tests, dependency policy,
licence headers, privacy/egress construction and provenance). All task seals validate. The temporary
PostgreSQL instances were shut down after verification; their files remain on the external disk.
Release `duet` and `duet-eval` binaries were rebuilt in `target-gate/release/` on the external disk.

## Continue from here

- X1's real default-hybrid misses are string continuation across a line comment (all three
  seeds) and explicit BETWEEN ASYMMETRIC (seed 3). The alleged ANY/ALL privacy gap is closed.
- Use revision 2 for any future X1 comparison; keep the original reports as historical evidence.
- M5.3, the local security auditor, is the next roadmap milestone. The open detector false-positive
  classes and visual Captain Comic check remain as recorded in the hand-off.
- Gate 2 still needs a new measurement before M5 because of earlier prompt changes. The final
  red-team pass still precedes benchmark publication.
- Build and scratch files remain on `/Volumes/EXT_DISK/duet_v2/`. Verification log:
  `/Volumes/EXT_DISK/duet_v2/x1-grader-gate.log`.
