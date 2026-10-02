# Captain Comic debug-log review — 2026-10-01

Reviewed all eight saved audit logs and eight transcripts in the local
`captain_comic_duet` workspace, through run `20261002-003636-031443` (UTC).
The review covers about 55.3 MB of audit data and 2.32 MB of transcripts. All
JSONL records parsed. Three subagents reviewed boundary/tool errors, provider
and image errors, and session/debugging behavior; the parent applied and
verified the combined fixes. Raw logs and private content remain local.

## What the logs contain

| Recorded outcome | Count | Interpretation |
| --- | ---: | --- |
| Requested tools / returned results | 245 / 244 | One interrupted write has no result. |
| Tool results beginning with `error:` | 19 | Breakdown below. |
| Commands: success / nonzero exit / held output | 50 / 38 / 2 | Held output is not labeled a command failure. |
| Frontier requests / usage response records | 223 / 220 | These counts alone do not establish provider reliability. |
| Apparent session failures that were normal exits | 4 | Exact legacy “session left open” sentinel. |
| Web operations | 26 | 23 transport successes, two boundary refusals, one unsupported PDF. |

The 19 tool errors were six exact-edit misses, one malformed edit request, two
unavailable image routes, two file paths used as handles, two historical
public-web false positives, two protected-value write refusals, one
outside-workspace read, one sensitive raw-read refusal, one unsupported PDF,
and one historical non-Git listing failure.

The 38 nonzero commands were 16 generated app/test failures, 11 JavaScript
syntax/reference/type failures, seven sandbox denials, three connection
failures, and one environment probe. Some failed commands are superseded by
later repairs in the same transcript. The latest run contains 17 of those
command failures and nine tool errors.

The 494 audit interventions are repeated sanitization of known values in
conversation history, not 494 independent defects. The 23 web transport
successes include two HTTP 403 responses and one HTTP 404 response. Remote
refusals are not successful research results.

One September 30 record reports usage for a failed provider attempt; the turn
eventually completed. The latest turn was interrupted. Retry reasons before
output are not persisted, so this review cannot establish that no retries or
transient provider errors occurred. A separate older run has a completed
turn but incomplete invocation-finalization evidence.

## Duet fixes

| Cause | Change | Regression coverage |
| --- | --- | --- |
| Normal session exits appeared as failures | `Terminal::Open`, successful exit code, readable history/TUI status; exact legacy sentinel normalized only when read | Serialization, actual-failure distinction, session resume, end hooks, audit and history |
| Per-edit paths and a missing top-level path caused retry loops | One-file edit schema and actionable validation before reading or modifying files | Missing/nested paths and empty batches |
| A failed edit batch suggested line numbers from uncommitted intermediate text | Recovery hints use the original file | Atomic rollback and persisted-line assertions |
| Denials and connection errors gave little recovery help | Explain `$TMPDIR`, supported file tools, child-process lifetime, unused allowed ports and browser-daemon restrictions | Native/runtime error casing, network modes, no private-output echo |
| Refused images were retried with invented handles | Explain that refusal creates no handle and give policy-respecting vision options | No model call on refusal; sensitive/protected routes stay blocked |
| File paths were passed to handle tools | Explain valid returned handle IDs without echoing arbitrary paths | `ask_local`, `read_raw`, `synthetic_sample` |
| Mock rasterization was treated as visual evidence | Prompt requires real application/browser captures and explicit limits for unavailable tools | Both prompt variants and deliberate prompt fingerprint |

Historical public-web false positives and non-Git listing failures were already
fixed in the current source. Their existing regression coverage is retained.
Expected privacy refusals remain enforced. PDF ingestion is still unsupported;
the agent must use an available source or report the limitation.

## Generated workspace fixes

- Fixed `dbg.js` calling nonexistent `findGroundY`; it now uses `groundAt`.
- Fixed the instructions screen rendering two `UNDEFINED` labels for separator rows.
- Replaced three mock-rasterizer entry points with one actual Chromium helper.
  Each run uses an isolated temporary profile, muted audio, a unique session,
  and cleanup. It captures the application's canvas in 15 prepared scenes and
  checks player movement and page exceptions. It downloads nothing.
- Hardened `tinyserver.js`: loopback binding, explicit public-asset allowlist,
  traversal/dotfile/symlink refusal, malformed-URL handling, bounded reads,
  validated ports, inert import, and a useful occupied-port error. The previous
  server could serve files outside its root and expose hidden workspace files.
- Preserved old simulated PNGs in `shots-legacy-mock` with a label. Preserved
  changed originals and their hashes in a private external backup. Audit logs
  and transcripts were not rewritten.

The workspace's `DEBUGGING.md` documents the repaired checks. Its new screenshots
and machine-readable report are in
`shots/browser-2026-10-02T01-40-42-131Z-802853e3/`.

## Verification

| Check | Result |
| --- | --- |
| Generated game's headless logic checks | 29 passed |
| Preview-server security groups | 4 passed |
| Actual loopback HTTP probe | Public assets served; traversal/private paths denied; malformed URL refused without crashing |
| Transition diagnostic | Successfully advances from zone 0 to zone 1 |
| JavaScript syntax / map diagnostic | 18 files passed; 24 zones parsed |
| Real Chromium scene checks | 15 passed; defined text and nonblank actual canvas |
| Real-browser movement / page exceptions | Passed / zero |
| Visual inspection | Title, gameplay and repaired instructions inspected |
| Duet unit/integration tests | 1,220 passed; 0 failed; 22 existing ignored tests |
| Embedded-program harness / doc targets | Passed / all 18 completed, no runnable examples |
| Formatting, Clippy, dependency policy, license, privacy/egress construction, provenance | All passed |

The complete gate exited 0 on macOS Apple Silicon with Rust/rustdoc 1.97.1:

```sh
CARGO_TARGET_DIR=/Volumes/EXT_DISK/duet_v2/target-gate \
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true bash tools/gate.sh
```

Local gate log: `/private/tmp/duet-debug-full-gate.log`.
SHA-256: `3574efd84bfcf49c835bda99b31de0d2a1a613fafa205cdc64360823fe503961`.
The ignored tests keep their existing live-service, fixture or benchmark requirements.

The optimized release was rebuilt and installed at the existing local command path,
`/Users/maximp/OpenCode/duet_v2/target/release/duet`. Its SHA-256 is
`01403009b75efa8eafa9938bfb693b6f20329154a4b3f02ab565881e4f9fc0bd`.
The installed executable read all eight history records: four Open, one Ready
to continue, three Completed. The four legacy summaries and their transcripts
were byte-for-byte unchanged after that read. Running sessions must be restarted
to load this binary; `duet --resume` continues the saved conversation. This was
a local installation, not a remote deployment.

An initial full gate could not create fixtures because the internal disk ran
out of space. Clearing 9.5 GiB of Duet's rebuildable debug cache restored space;
builds use the external target directory. Later runs caught stale test assertions
for ordinary session exits; these now expect `open`, preserving assertions for
actual failures. No test or sandbox policy was disabled.

These are diagnostic and regression checks, not a claim of full gameplay
correctness, original-game fidelity, or end-to-end browser access from inside
Duet's command sandbox. The browser verification was run by the operator's
coding assistant outside that sandbox. No new paid model task or paired
quality/cost benchmark was run.

The one-shot prompt fingerprint is now
`51c6b540bb769fb80043070a3bd2cfb02512838762a609aacbe6f0d286b48160`.
Earlier benchmark results do not validate this prompt revision. See
[value evidence](../VALUE_EVIDENCE.md) for measured quality, security and
cost results and their limits.
