# Code review and performance refactor — 2026-10-01

Three subagents reviewed the provider/configuration layer, privacy/review engine,
and terminal UI. The main agent reviewed conversation handling and private storage,
sampled supporting network/framing code, integrated the changes and ran the final
checks. Reviewers also checked each other's changes. Existing work in this checkout
was preserved; this report covers the changes from this review, not every difference
from Git HEAD (`b853842`).

## Findings fixed

| Area | Finding | Change |
|---|---|---|
| Local endpoint trust | The hand-written URL parser disagreed with the HTTP client about backslashes, userinfo and whitespace. A remote endpoint could be classified as local. | Use the HTTP client's URL parser for host, port and transport decisions. Regression cases cover deceptive URLs and normalized loopback addresses. |
| Provider redirects | A validated provider endpoint could redirect prompt bodies or probe credentials to another destination. | Disable redirects in provider requests and context probes. Real loopback HTTP tests verify that the redirected destination receives no request. Probes also ignore unsuccessful model listings. |
| Log repair | Repair followed a leaf symlink and reopened the pathname before truncating a torn tail. FIFOs could hang reads. | Open without following leaf links, require a regular file, and read/repair through the same descriptor. Preserve intact read-only logs through a read-only fallback. |
| Security review | Each finding reparsed and walked the same source for related declarations. | Reuse the original syntax tree and collect declarations once per file; retain separate context limits for each finding. |
| Copied-content filtering | Every overlapping token window allocated lowercase strings again; redaction built temporary window and hit vectors. | Normalize each token once, share indexing work and stream matching windows. Original byte ranges, thresholds and public-text exemptions remain the same. |
| Configuration | Each reload reparsed immutable registry defaults as TOML. | Cache only the registry defaults. Owner/project files and policy remain fresh on every load. |
| Conversation context | The run and compaction paths repeatedly estimated the same unchanged conversation. The ledger duplicated the text-sizing function. | Reuse the already computed estimate within each operation and share the ledger's sizing helper. Public helpers still compute their own estimate. |
| Subagent transcripts | Wrapping each entry cloned its full payload before serialization. | Serialize a borrowed wrapper, preserving the on-disk replay format byte for byte. |
| Terminal redraws | Audit records were reparsed on every draw; whole cached conversations and offscreen diff rows were materialized repeatedly. | Cache audit summaries/selected detail, copy only the requested conversation range, and format only visible diff rows. |

## Measurements

Local synthetic measurements on macOS 27.0.1, ARM64, Rust 1.97.1. These measure
individual operations; they do not establish whole-task latency, model quality or
API cost savings. Independent workloads must not have their speedups multiplied.

| Operation and workload | Before | After | Before / after |
|---|---:|---:|---:|
| Review: 40,020-byte Python source, 24 findings, ten scans; release | 699.096 ms | 45.624 ms | 15.32× |
| Copied-content index: 268,890 bytes; `rustc -O` | 8.117 ms | 3.080 ms | 2.64× |
| Redact that input: 30 normal + 30 strict calls; `rustc -O` | 245.340 ms | 151.745 ms | 1.62× |
| Warm configuration reload: 2,000 loads, per-load time; release | 457.8 µs | 380.2 µs | 1.20× |
| Request context check: 32 turns × two 4 KiB results, 300 checks; release | 55.01 ms | 27.43 ms | 2.01× |
| Request context check: 128 turns × two 16 KiB results, 300 checks; release | 787.95 ms | 385.49 ms | 2.04× |
| Cached transcript: 7,999 total rows, 30 visible, 100 preparations; debug | 179.534 ms | 45.449 ms | 3.95× |

Review and overlap values are medians of three samples. Context values are medians
of five samples with alternating before/after order. A second configuration run
measured 374.2 µs/load; the table uses the first, more conservative result.
Concurrent machine load varied. Review's before-source snapshot was not retained;
the before timings are recorded observations from this session, not a separately
replayable baseline. The context and UI benchmarks include both old and new paths.

An allocation-free JSON-counting experiment was removed after repeated measurements
showed a slowdown for large contexts. The final context refactor retains the
original serializer and reduces the number of full passes.

Commands for the checked-in benchmarks:

```sh
cargo test --release -p duet-review --test throughput -- --ignored --nocapture
cargo test --release -p duet-boundary --test throughput copied_span_matching -- --ignored --nocapture
cargo run --release -p duet-config --example config_load_benchmark
cargo test --release -p duet-agent --lib request_masking_throughput -- --ignored --nocapture
cargo test -p duet-tui benchmark_cached_transcript_viewport -- --ignored --nocapture
```

The checked-in overlap benchmark exercises the same workload through Cargo; the
recorded overlap measurements used an isolated optimized driver. Measurements and
raw review/overlap samples are also in [the evidence JSON](../benchmarks/refactor-2026-10-01.json).

## Verification and limits

- Boundary/review: 310 tests passed, including canary, outbound agreement, property,
  Unicode and review-context checks. An additional temporary comparison driver
  matched 10,000 old/new normal and strict redaction outputs.
- Provider/configuration: 121 tests passed, including real HTTP redirect regressions,
  endpoint normalization, fresh file reloads and policy tightening.
- TUI: 91 tests passed, including viewport ranges, scrolling, resizing, Unicode
  selection, cache refresh and long diffs.
- Filesystem: 16 tests passed, including read-only logs, symlinks, FIFOs and torn writes.
- Agent: 81 tests passed. Six command/protected-edit tests fail because the execution
  environment refuses nested macOS sandbox creation (`sandbox_apply: Operation not
  permitted`). Live and manual measurement tests remain intentionally ignored.
- Workspace Clippy with `-D warnings`, formatting, license headers, privacy/egress
  construction checks, provenance and `git diff --check` passed.
- Dependency policy passed offline using a temporary writable copy of the existing
  advisory database; the global Cargo directory is read-only in this session.
- The first full gate attempt exhausted disk space while linking CLI integration
  binaries. After Cargo cleanup, the workspace test build completed successfully
  with incremental compilation disabled and two build jobs. The test run stops on
  the same six sandbox-dependent agent tests, so the full gate is **not green**.

Both `target/debug/duet` and the optimized `target/release/duet` were rebuilt.
The release binary passed `--version` and `--help` smoke checks. No provider calls
or model charges were needed for these checks. Run from the repository with
`./target/release/duet`, or use its absolute path from another working directory.

The Privacy panel still rereads its audit file when its size changes; safely tailing
partial/replaced logs remains a separate improvement. Provider environment-proxy
behavior is unchanged, so the redirect fixes are not a claim about every possible
network route. Log hardening here covers the leaf and descriptor used for repair;
it is not a redesign of all private-directory traversal.
