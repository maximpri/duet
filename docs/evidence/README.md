<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Evidence and historical records

The [documentation index](../README.md) leads to current guides. This directory
retains the results, failures and source identities behind Duet's published
claims. A dated result describes its recorded build and environment, not every
later commit.

## Benchmark results

| Record | What it contains |
| --- | --- |
| [October 4 comparison](benchmark-54-2026-10-04/README.md) | Current 54-outcome report, 27 pairs, audit verdict, provenance and checksums; includes the externally stopped zero-score outcome |
| [Earlier stopped batch](release-hardening-2026-10-03/benchmark/README.md) | Original interrupted batch and retained budget accounting |
| [September small-to-large report](benchmarks/m52-sl-report.md) | Historical paired quality, disclosure, cost and timing results |
| [September large-repository report](benchmarks/m52-xl-report.md) | Historical quality-goal failure and its measured outcomes |
| [X1 grader correction](reviews/x1-grader-audit-2026-09-30.md) | PostgreSQL validation, corrected hidden grader and regrading of saved artifacts |

The [value evidence guide](../VALUE_EVIDENCE.md) explains how these batches
differ. Frozen reports, manifests and source records keep their original bytes.

## Actual Duet runs and captures

The [demonstration guide](../launch/DEMO.md) brings together the task, direct
Terminal screenshots, reproduction steps and offline verification commands.

| Packet | What it establishes |
| --- | --- |
| [Original launch records](launch-2026-10-01/manifest.json) | Before/intermediate/after disclosure counts, code changes, local-only task and the audit-tampering negative control |
| [Follow-up billing run](launch-refresh-2026-10-01/manifest.json) | Four passing tests and no complete planted-value matches in five recorded frontier requests |
| [Native Terminal capture](readme-native-2026-10-02/manifest.json) | Unmodified screenshots and the color GIF used in the README |
| [Earlier terminal replay](readme-color-2026-10-02/manifest.json) | Archived replay source; distinct from native screenshots |

Capture media remain at their recorded paths because the manifests bind their
hashes. Audit integrity and a zero literal-value count answer different
questions; neither alone proves complete privacy.

## Security, build and distribution checks

- [Release-hardening record](release-hardening-2026-10-03/README.md): model
  endpoints, sandbox cancellation, Linux checks and offline corresponding source.
- [Publication review](publication-review-2026-10-02/README.md): earlier platform
  and source-package checks.
- [Licensing evidence](licensing-2026-10-02/offline-source-validation.json):
  source packaging; [X1 notice changes](licensing-2026-10-02/X1-notice-changes.json)
  preserve upstream modification notices and seal history.
- [Security-auditor review](reviews/security-auditor-2026-09-30.md): measured
  coverage and known limits of the auditor and repository scanner.
- [Code review](reviews/code-review-2026-10-01.md): historical performance and
  refactoring measurements.

The root [security document](../../SECURITY.md#known-limits) retains the threat
model, known limits and advisories. Current release decisions belong in
[publication readiness](../PUBLISH_READINESS.md).

## Earlier planning and investigations

Superseded plans, session handoffs, campaign drafts and game-demo material were
removed from the active documentation. Their complete content remains in
[the pre-cleanup Git snapshot](https://github.com/maximpri/duet/tree/9b0ac104ba6947e338ca3baacfd31de2c2823e83/docs).

- [Original goals and success criteria](https://github.com/maximpri/duet/blob/9b0ac104ba6947e338ca3baacfd31de2c2823e83/docs/TARGET_STATE.md#success-criteria-pre-registered-passfail)
  and [acceptance ledger](https://github.com/maximpri/duet/blob/9b0ac104ba6947e338ca3baacfd31de2c2823e83/docs/ACCEPTANCE.md)
  preserve the planned gates and earlier verdicts. The current mechanical
  benchmark does not establish the original judged quality goal.
- [Session investigations](https://github.com/maximpri/duet/blob/9b0ac104ba6947e338ca3baacfd31de2c2823e83/docs/HANDOFF-2026-09-29.md)
  retain X2 follow-ups, cost experiments and their limitations.
- [Captain Comic review](https://github.com/maximpri/duet/blob/9b0ac104ba6947e338ca3baacfd31de2c2823e83/docs/CAPTAIN_COMIC_REVIEW.md)
  retains the incomplete-game findings, original screenshots and artifact hashes.
  Its [task-specific checker](https://github.com/maximpri/duet/blob/9b0ac104ba6947e338ca3baacfd31de2c2823e83/tools/check-captain-comic.mjs)
  is available in the same snapshot; it is not part of Duet's current test suite.

Historical source manifests describe their pinned revisions. A file removed
from the current checkout can still be obtained at its recorded revision;
cleanup does not rewrite the earlier manifests or turn their checks into a
verification of the current tree.
