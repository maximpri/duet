# Commit validation — 2026-10-01

Documentation was reconciled with the current terminal implementation: command discovery,
keyboard/mouse navigation, clipboard workers, image queues, draft recovery and verification limits.
Relative Markdown link targets in the eight checked README/architecture/usage/acceptance/handoff/
evidence documents exist. `git diff --check` passed.

## Required full gate attempt

Command:

```sh
CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 bash tools/gate.sh
```

- Formatting: passed.
- Full-workspace Clippy, all targets, warnings denied: passed.
- Agent unit tests: **97 passed, 6 failed, 2 ignored**.
- All six failures reported `sandbox-exec: sandbox_apply: Operation not permitted` in this
  execution environment. The gate stopped with exit 101; later suites and dependency policy
  were not reached. The full gate is **not passing** in this environment.

Affected tests:

- `protected::tests::protected_edits_are_implemented_locally_and_checked`
- `tools::sensitive_command_tests::commands_cannot_read_git_history_or_run_state`
- `tools::sensitive_command_tests::commands_cannot_read_sensitive_files_unless_their_output_stays_local`
- `tools::sensitive_command_tests::sensitive_commands_never_reach_run_state_and_resolve_placeholders_locally`
- `tools::sensitive_command_tests::what_a_sensitive_command_writes_into_build_output_stays_sensitive`
- `tools::tests::finish_requires_the_acceptance_command_to_pass_after_a_repair`

No test or sandbox policy was disabled to manufacture a passing result. Run the unchanged full
gate on a host that permits Duet's sandbox before treating this checkpoint as release-validated.

## Earlier targeted checks

- Latest command-menu/TUI suite: **122 passed**, 1 manual benchmark ignored.
- CLI unit suite: **77 passed**.
- Image policy and file-read regressions: **6 passed**.
- TUI/CLI/agent all-target Clippy and the repository fast gate: passed.
- Release build and attachment line-mode smoke test: passed.
- Full-screen terminal and real desktop clipboard integration: pending, as documented in
  [TUI validation](tui-validation-2026-10-01.md) and [menu validation](command-menu-validation-2026-10-01.md).

These targeted checks do not replace the full gate.

## Commit and push attempt

The initial documentation staging command succeeded for nine files. Staging the remaining
source and evidence changes was denied while creating `.git/index.lock` (`Operation not
permitted`). No commit was created. A remote-head lookup also failed because `github.com`
could not be resolved, so no push was completed. The working tree and partial documentation
index remain intact for completion in an environment with writable Git metadata and network access.
