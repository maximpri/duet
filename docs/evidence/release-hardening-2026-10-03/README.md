# Release hardening verification — October 3, 2026

The [manifest](manifest.json) records the changed product/tool source hashes and
base revision. The [macOS gate log](macos-gate.log.gz) retains the combined check:
1,278 tests passed, 22 environment-dependent tests ignored; formatting, Clippy,
dependency/license policy, SPDX headers, privacy/egress construction and
provenance passed. Only local workspace, home and cache paths were normalized.

Two initial gate failures were corrected before this passing run: a timestamp
window overclassified an untouched build file, and automatic privacy previews
changed piped session output. Before/after metadata comparisons now cover build
files, and automatic previews are limited to interactive terminals.

The planned paid benchmark and Linux x86-64 full-system verification are still
in progress. They are not included in this gate result. See
[publication readiness](../../PUBLISH_READINESS.md) for the current scope.
