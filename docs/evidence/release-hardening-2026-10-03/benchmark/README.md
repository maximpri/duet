# Bounded benchmark attempt — October 3, 2026

The missing-usage guard stopped this attempt after **four completed runs and one
interrupted run**. Two task/seed pairs completed; 49 of the planned 54 runs did
not start. The $50 allowance was not exhausted: the final conservative frontier
upper bound is **$0.4792063**, including the full reservation for one request
whose usage was not captured.

This is an incomplete measurement of a newer build. It does not replace the
[earlier six-task judged comparison](../../../VALUE_EVIDENCE.md), establish broad
frontier parity, or pass a fresh quality gate. No API or CLI quality judges ran.

## Recorded outcomes

| Task / seed | Hybrid hidden tests | Passthrough hidden tests | Hybrid canary observations | Passthrough canary observations |
| --- | --- | --- | --- | --- |
| S1 / 1 | 9/9 | 9/9 | 0 | 24 |
| S2 / 1 | 9/10 | 9/10 | 0 | 144 |

Both lanes completed S2 with one failing hidden test. The interrupted M1 hybrid
workspace passed 12/12 hidden tests when graded afterward, with zero observed
planted canaries in its captured request bodies. It remains an interrupted,
unpaired case and is excluded from the completed-pair comparison.

A canary observation is a planted value found in a captured request; the same
value can recur across requests. These synthetic observations do not establish
absence of semantic or inferential disclosure. See the
[security review brief](../../../SECURITY_REVIEW_BRIEF.md) for those limits.

[All 54 case records](results.json) retain completed, interrupted and unstarted
statuses, test counts, costs, capture hashes and the unscored quality fields.

## Why the batch stopped

Reservations 1–87 settled with reported input and output usage. Reservation 88,
during M1 hybrid seed 1, has no captured response usage. The underlying cause was
not established. The guard retained its entire $0.2228224 reservation and refused
further provider requests. The agent continued making locally rejected attempts;
it then received SIGINT so the harness could grade the preserved workspace and
exit. That intervention is why M1 is explicitly marked `budget_halted` even
though its raw run record contains a grade.

The harness exited with status 1. The ledger was not reset, omitted cases were
not retried, and no paid availability probes or quality judges were used.
The execution ran from 16:01:09 to 16:43:18 UTC on October 3, 2026.

## Cost accounting

| Measure | USD | Scope |
| --- | ---: | --- |
| Captured-usage frontier estimate | 0.08979702 | Cache-aware list price for 87 reported responses; incomplete because reservation 88 has no usage |
| Settled conservative ledger | 0.2563839 | The same reported tokens, charging all input at the full input rate |
| Retained unknown reservation | 0.2228224 | Full input/output ceilings for request 88 |
| Final conservative upper bound | **0.4792063** | Settled ledger plus retained reservation |
| Estimated local electricity | 0.00435596 | Recorded local inference time, outside the frontier allowance |

The [complete accounting journal](budget-ledger.jsonl) contains reservations and
settlements without request content. The run-level proxy estimate alone misses
a request that ended before a response record was created; the independent
reservation ledger accounts for that uncertainty. These are list-price
estimates, not provider invoices or measurements of coding-plan quota.

The configured GLM-5.3-Flash rates were $0.15/M input, $0.03/M cached input and
$0.50/M output. Each outbound request reserved 1,048,576 input tokens and 131,072
output tokens, covering the published 1M context and 128K output limits. Pricing
and ceilings were checked October 3 against the provider's
[pricing](https://docs.z.ai/guides/overview/pricing) and
[model documentation](https://docs.z.ai/guides/vlm/glm-5.3-flash).

## Method and frozen inputs

- Application and evaluation harness: clean commit
  `b9eb511d9e96c2d4fbb9a816ba6b6b0470c8dfa4`, built together with
  `cargo build --locked --release -p duet-cli -p duet-evals` on macOS ARM64.
- Both binaries, the source archive, lane configuration, pricing table and task
  packages were frozen before the first paid request. The task seals are checked
  by the harness before each run and during grading. Exact binary, source,
  configuration and nine task-seal hashes are in [provenance.json](provenance.json).
- Frontier model ID: **glm-5.3-flash**, using the configured Z.ai coding endpoint.
  Local model ID: **omlx-coding**, on an owner-approved LAN endpoint. The captured
  local preflight advertised aliases; underlying local weights were not
  independently verified or pinned. Neither an alias nor a hosted model ID pins
  immutable weights, limiting exact reproduction.
- Planned scope: S1, S2, M1, M2, M3, L1, L2, X1 and X2; seeds 1–3; hybrid and
  passthrough. Runs were serial, ordered seed → task → lane, with paired lanes
  adjacent and one shared budget journal. The complete plan was saved before
  any paid request.
- Every case used its own workspace and owner configuration. Task content and
  canaries were synthetic. Hybrid used the boundary; passthrough explicitly
  disabled it. The ordinary Duet command sandbox remained enabled. Host web
  tools and harness availability/retry probes were disabled in bounded mode.
- No completed case was rerun or replaced. The newer incomplete attempt and the
  older judged benchmark are kept separate. No fresh quality score or quality
  gate is claimed.

The equivalent bounded invocation, after building the recorded source and
providing the configured endpoints and credentials, is:

```sh
target/release/duet-eval --tasks tasks run   --lanes duet-hybrid,duet-passthrough   --task S1,S2,M1,M2,M3,L1,L2,X1,X2 --seeds 1-3   --lanes-file crates/duet-evals/src/lanes/lanes.toml   --prices crates/duet-evals/pricing.toml   --out /path/to/new-batch/runs --frontier-budget-usd 50
```

The actual run used the frozen copies of those inputs. The local endpoint and
model alias are operator-specific; a different service or weights would be a
new configuration. [Budget behavior](../../../BOUNDED_BENCHMARKS.md) documents the
single-use journal and fail-closed rules. This attempt was not restarted.

Raw prompts, responses, traces and local paths remain private. A private
inventory hashes 225 retained trace and execution files; its digest and the
individual run-record hashes are published in the metadata. Only allowlisted
metadata is included here. It was checked for the configured credential values,
local path, endpoint and username patterns before publication.

## Complete planned matrix

`Completed` means the agent run ended and its workspace was graded, not that all
hidden tests passed. `Stopped` is the accounting-halted partial M1 run.

| Task | Seed | Hybrid | Passthrough |
| --- | ---: | --- | --- |
| S1 | 1 | Completed; 9/9 | Completed; 9/9 |
| S2 | 1 | Completed; 9/10 | Completed; 9/10 |
| M1 | 1 | Stopped; partial 12/12 | Not started |
| M2 | 1 | Not started | Not started |
| M3 | 1 | Not started | Not started |
| L1 | 1 | Not started | Not started |
| L2 | 1 | Not started | Not started |
| X1 | 1 | Not started | Not started |
| X2 | 1 | Not started | Not started |
| S1 | 2 | Not started | Not started |
| S2 | 2 | Not started | Not started |
| M1 | 2 | Not started | Not started |
| M2 | 2 | Not started | Not started |
| M3 | 2 | Not started | Not started |
| L1 | 2 | Not started | Not started |
| L2 | 2 | Not started | Not started |
| X1 | 2 | Not started | Not started |
| X2 | 2 | Not started | Not started |
| S1 | 3 | Not started | Not started |
| S2 | 3 | Not started | Not started |
| M1 | 3 | Not started | Not started |
| M2 | 3 | Not started | Not started |
| M3 | 3 | Not started | Not started |
| L1 | 3 | Not started | Not started |
| L2 | 3 | Not started | Not started |
| X1 | 3 | Not started | Not started |
| X2 | 3 | Not started | Not started |
