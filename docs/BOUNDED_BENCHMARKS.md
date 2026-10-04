<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Running a benchmark with a spending limit

`duet-eval run --frontier-budget-usd 50` enforces a conservative aggregate
list-price allowance at the model proxy, before each outbound request. This mode
currently supports the configured Duet lanes using GLM-5.3-Flash at the Z.ai
coding endpoint. The cap cannot exceed $50. Local inference electricity is
reported separately and is not part of the frontier allowance.

The fixed reservation covers 1,048,576 input tokens and 131,072 output tokens per
request ($0.2228224 at the verified rates). These ceilings conservatively cover
the provider's published 1M context and 128K output limits. Input, cache and
output rates must match the verified pricing entry; unknown or changed prices
stop the batch. Sources, checked October 3, 2026:
[model limits](https://docs.z.ai/guides/vlm/glm-5.3-flash) and
[pricing](https://docs.z.ai/guides/overview/pricing).

Every request, including an agent retry, reserves its full amount in a synced
`budget.jsonl` journal before it is sent. Concurrent requests share the same
allowance. A completed response with both input and output token counts releases
the unused portion; cached input is conservatively charged at the full input
rate in this journal. Detailed benchmark reports may show the lower cache-aware
list price. Provider errors, missing usage, interrupted streams and dropped
connections retain the entire reservation and stop further paid requests.

The proxy permits only text chat and function tools. It rejects WebSockets,
multimodal requests, multiple completions and unpriced request options. Redirects,
environment proxies and HTTP client retries are disabled. Bounded runs disable
host web tools, automatic provider-availability probes and harness quota retries.
Duet's ordinary command sandbox remains active. The allowance is enforced for
model traffic routed through this proxy; it is not an account-wide provider cap
or a protection against an owner modifying the harness or lane configuration.

Runs are scheduled seed, then task, then lane: each seed covers every task
before the next seed starts, with paired lanes adjacent. `planned-runs.json`
records the full requested matrix before any paid request, including cases left
unstarted if accounting becomes unknown or the allowance is exhausted.

The journal is single-use. A restart refuses an existing journal rather than
silently resetting charges or guessing what an interrupted provider request cost.
Keep the entire batch, including unsuccessful runs and its journal. Do not delete
the journal to resume a paid batch. A separately authorized continuation must
preserve every prior charge and retained reservation and allocate only the
remaining aggregate allowance. On a coding-plan subscription these are list-price
token estimates, not a prediction of an additional API invoice or subscription
quota.

Example (put the batch and frozen binaries on a disk with enough room):

```sh
/path/to/frozen/duet-eval run \
  --lanes duet-hybrid,duet-passthrough \
  --task S1,S2,M1,M2,M3,L1,L2,X1,X2 --seeds 1-3 \
  --out /path/to/new-batch/runs --frontier-budget-usd 50
```

Freeze `duet` beside `duet-eval`, record both binary hashes, the source revision
and uncommitted source snapshot, lane definitions, pricing file and task seals.
The harness resolves the adjacent Duet binary. Preserve the planned task/seed/lane
matrix so omitted or interrupted runs cannot disappear from the reported sample.

## Recorded October 2026 continuation

The [54-outcome comparison](evidence/benchmark-54-2026-10-04/README.md) used
separately reviewed external controllers and a fixed final single-case
continuation. This was an evaluation procedure, not a shipped automatic-resume
feature. The original journal and all attempts remain preserved; conservative
accounting totals $23.55011430, including $0.44564480 retained for unknown usage.

Collection combined serial execution with at most one hybrid and two passthrough
workers. A naturally completed orphaned case was recovered without rerunning it;
its unavailable parent wait status and incomplete earlier descendant history
remain disclosed. X1 hybrid seed 3 received a separately documented external
deadline disposition, counts zero and was not rerun. No native timeout, terminal
or grade was fabricated.

Final verification reconciles all attempt journals and captures and checks
process, writer and lease quiescence. Mixed scheduling makes these timings
unsuitable for a controlled lane-latency comparison.
