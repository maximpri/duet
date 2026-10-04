# Release hardening verification — October 3, 2026

The [final manifest](final-macos-manifest.json) records source commit `ae66b4a`
and the product/tool file hashes. The [final macOS gate log](final-macos-gate.log.gz)
retains 1,278 tests passed, 22 environment-dependent tests ignored; formatting,
Clippy, dependency/license policy, SPDX headers, privacy/egress construction and
provenance passed. Only local workspace, home and cache paths were normalized.

The [earlier manifest](manifest.json) and [gate log](macos-gate.log.gz) preserve
the first combined pass before the cancellation and packaging follow-ups.

Two initial gate failures were corrected before this passing run: a timestamp
window overclassified an untouched build file, and automatic privacy previews
changed piped session output. Before/after metadata comparisons now cover build
files, and automatic previews are limited to interactive terminals.

The [Linux x86-64 record](linux-x86/README.md) retains the cancellation failure,
unchanged passing regression, complete-kernel sandbox checks and Rust 1.90
optimized build at `b9eb511`. Commit `ae66b4a` changes only archive packaging and
its tests from that application source: macOS filesystem metadata is excluded,
with real-xattr regressions for source and binary archives.

The [offline source record](offline-source/README.md) verifies the final
`1e4f2e4` archive against a successful frozen build from `ae66b4a`; their build
inputs and vendored dependencies are identical. The final archive also removes
the tracked link to local experiment storage.

The [bounded benchmark attempt](benchmark/README.md) ran from the frozen
`b9eb511` application. Four cases completed, M1 hybrid was interrupted after
missing response usage halted the accounting guard, and 49 cases did not start.
The final conservative frontier upper bound is $0.4792063 against the $50
allowance. All 54 planned statuses, binary/input hashes and the content-free
budget journal are retained; fresh quality scores remain unscored. The older
judged comparison is separate evidence. See [publication readiness](../../PUBLISH_READINESS.md)
for remaining work.

## Later continuation evidence

The [October 4 final comparison](../benchmark-54-2026-10-04/README.md) carries
this stopped attempt into a complete record of 54 outcomes and 27 pairs. It
preserves the original valid cases, interrupted attempt, retained charge and
subsequent history. The record contains 53 native records and grader records,
plus one externally adjudicated zero-score product failure without native
terminal artifacts.

The benchmark application remains `b9eb511d9e96c2d4fbb9a816ba6b6b0470c8dfa4`.
The additive packet records its own final verification, accounting and execution
limitations; it neither replaces this historical packet nor supplies fresh
quality judgments.
