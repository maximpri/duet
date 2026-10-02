# Commit validation — 2026-10-01

**Final result: the complete `tools/gate.sh` passed with exit 0.** The Git metadata and
network restrictions from the initial attempt were resolved by the updated execution permissions.

Documentation was reconciled with command discovery, keyboard/mouse navigation, clipboard
workers, image queues, draft recovery and verification limits. Relative Markdown link targets
in the updated README, architecture, usage, acceptance, plan, handoff and evidence documents
were checked. Both working-tree and staged whitespace checks passed.

## Complete gate

```sh
CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 bash tools/gate.sh
```

Platform: macOS, Apple Silicon; Rust and rustdoc 1.97.1 (Homebrew).

| Check | Result |
|---|---|
| Workspace and fuzz formatting | Passed |
| Workspace Clippy, all targets, warnings denied | Passed |
| Unit and integration tests | **1,208 passed, 0 failed, 22 ignored** |
| Embedded-program integration harness | Passed |
| Documentation tests | All 18 crate targets completed; no runnable examples |
| Dependency advisories, bans, licenses and sources | Passed |
| Source license headers | Passed |
| Privacy and egress construction checks | Passed |
| Provenance | Passed |

The ignored tests retain their existing live-service, fixture or manual-benchmark requirements.
No test or sandbox policy was disabled. The gate ends with `gate: all checks passed`.

Local log: `/private/tmp/duet-precommit-verified-2026-10-01.log`.

Log SHA-256: `a3168178f129698457949153ad52d717d9abe71d86a67f337b015375b3080f87`.

## Issues resolved during the retry

The initial restricted attempt stopped with six `sandbox_apply: Operation not permitted`
failures (97 agent tests passed, 2 ignored). Staging failed at `.git/index.lock`, and GitHub
DNS resolution failed. All three restrictions cleared with the updated permissions.

The unrestricted runs exposed four stale assertions, corrected with subagent review:

- Explorer resume now checks **four started requests and three completed responses**, including
  the interrupted request and its retained accounting. The interrupt waits for the hanging mock
  request instead of an arbitrary 500 ms delay.
- The Anthropic preset assertion follows the configured Sonnet model.
- The local preset expects three audit records, checking each key, value, owner scope and
  confirmation, including the added API-key setting.
- The unknown-price assertion follows the current error message while retaining the exact
  known-alias cost/model checks and proof that an unknown model sends no request.

These corrections changed tests only. The targeted explorer, setup/doctor and top-clearance
suites passed with 11, 15 and 5 tests respectively.

One full run then reached documentation tests but encountered stale dependency artifacts
(`E0463`). A unified `cargo test --offline --jobs 2 --workspace --locked --doc` with
`CARGO_INCREMENTAL=0` rebuilt the affected libraries and passed. The final complete gate above
then passed without concurrent builds. No source or gate-policy change was needed for this issue.

## Terminal verification and remaining limits

The final gate includes `the_workspace_on_a_terminal`: Duet runs in a real pseudo-terminal
against a scripted frontier, receives a message, displays the reply and changed-file diff,
opens and closes settings, then restores the terminal on exit.

The release line-mode attachment smoke test also passed earlier. Actual desktop clipboard
checks, a fresh live screenshot and remote deployment remain outstanding. Clipboard adapter
tests use mock helpers; they do not certify a live Linux desktop. No paid model benchmark was
rerun. See [TUI validation](tui-validation-2026-10-01.md),
[menu validation](command-menu-validation-2026-10-01.md) and
[value evidence](../VALUE_EVIDENCE.md) for their separate scopes and limits.

## Repository checkpoint

Git staging and GitHub fetch succeeded on retry. The remote main branch was an ancestor of the
local branch, so no merge or force push was required. The checkpoint includes the reviewed source,
documentation, screenshots and evidence together. Git history and the remote branch identify
its published commit; earlier failed attempts created no commit.
