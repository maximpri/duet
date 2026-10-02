# Extension and instruction validation — 2026-10-01

Local development checkout on macOS ARM64. These results validate the extension
implementation; they do not establish model-quality parity or a cost-saving ratio.

| Check | Result | What it covers |
|---|---:|---|
| CLI library | 67 passed | Real in-process tool loop, first read/write/edit ordering, same-response mutation deferral, initial command recovery, plugin lifecycle, content integrity and policy limits |
| Agent library | 91 passed, 6 failed, 2 ignored | Instruction scope, overrides, limits, replay, skill privacy, explicit authorization, pagination, subagent propagation and existing agent tests |
| Extension catalog | 7 passed | Portable metadata, precedence, changed instructions, UTF-8, bounds and symlink/traversal rejection |
| Configuration library | 25 passed | Validation, owner/project precedence and organization policy bounds |
| Filesystem library | 16 passed | No-follow access, private state, atomic writes, locks and registered paths |
| Release CLI lifecycle smoke | 15 commands passed | Inspect, install, list, show, disable, enable, tamper refusal, disabled-corrupt isolation unregister, aliased configuration directories and invalid-first-command recovery |
| Strict Clippy | Passed | Agent, CLI and extension crates, all targets, warnings treated as errors |
| Fast repository gate | Passed | Formatting, licensing, provider/egress boundaries and provenance checks |

The agent library's six failures are command/sandbox-dependent tests in this
managed environment, where nested macOS sandbox execution reports
`sandbox-exec: sandbox_apply: Operation not permitted`. Two existing manual/live
tests were ignored. These are not counted as passes. The full repository gate
has not passed here.

The CLI smoke test used an isolated temporary configuration and workspace. It
checked snapshot directories at mode 0700 and files at 0600, rejected a modified
installed command, verified that a disabled corrupt snapshot does not block
skill discovery, and confirmed that unregistering preserves its recovery copy.
It also confirmed that disabled or invalid initial workflow commands do not initialize
a run. It made no model requests and did not modify the owner's installed plugins.

## Reproduce the automated checks

```sh
CARGO_INCREMENTAL=0 cargo test -p duet-cli --lib --offline --jobs 2
CARGO_INCREMENTAL=0 cargo test -p duet-agent --lib --offline --jobs 2
CARGO_INCREMENTAL=0 cargo test -p duet-config -p duet-extensions -p duet-fs --lib --offline --jobs 2
CARGO_INCREMENTAL=0 cargo clippy -p duet-cli -p duet-agent -p duet-extensions --all-targets --offline --jobs 2 -- -D warnings
tools/gate.sh --fast
CARGO_INCREMENTAL=0 cargo build -p duet-cli --bin duet --release --offline --jobs 2
```

The tests use in-process provider transports. They do not require provider keys
or billable model calls. Sandbox-dependent tests need a host that permits the
platform sandbox.

## Benchmark boundary

Skill descriptions are loaded first; full instructions and resources load only
when requested. Initial skill metadata is capped at 8 KiB, with paginated later
listing. This bounds initial context growth. Actual latency, task quality and
token savings still need paired task evaluations for this prompt revision;
prior Duet benchmark results must not be presented as measurements of this change.

See [supported formats and limits](../EXTENSIONS.md).

## Built artifact

- Binary: `target/release/duet` (macOS ARM64 release build).
- SHA-256: `8e3a7b21a939aa8d0225816c17500894975fad6824f68552fe3256ae7563144a`.
- Verified locally; this record does not claim a remote deployment.
