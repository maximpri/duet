# Linux x86-64 validation

The [record](platform-validation.json) distinguishes user-mode container checks
from a complete x86-64 Linux guest under QEMU on an ARM64 host. No physical
x86 hardware was tested. The guest used kernel seccomp and network namespaces.

- Rust 1.90 built the optimized application at `b9eb511`, checked the evaluation
  harness, and passed all 11 release tests.
- The full guest passed 17 setup/doctor tests. Its initial sandbox run reproduced
  the cancellation race: 28 of 29 tests passed.
- After freezing process ancestors before teardown, the unchanged 29 sandbox
  tests and six network tests passed. A single-thread TCG rerun also passed,
  avoiding the multi-thread emulator's memory-ordering warning.

The record retains both failures and fixes, exact source manifests, compiler
image, kernel and binary hashes, emulator arguments and guest initialization.
The first user-mode run and early guest environment issues are not full passes.
The tested sandbox prototype matches the committed source after formatting.

Logs and source manifests are gzip-compressed without changing their contents.
Names and hashes in `platform-validation.json` and `SHA256SUMS` refer to the
**decompressed** files. Read them with `gzip -dc <file>.gz`. Only machine-specific
paths were normalized in logs; original and normalized hashes are retained.
