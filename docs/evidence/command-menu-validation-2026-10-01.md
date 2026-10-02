# Command menu follow-up — 2026-10-01

The reported menu showed a fixed ten-row window with only `1/29` as its scroll cue.
Commands beyond the first ten existed but were difficult to discover.

The menu now uses the height available above the message input. Tall terminals show
all 29 commands; smaller terminals show a range, counts above/below, and a scrollbar.
Help, goals, history, status, models and settings appear first. Arrow keys, Page Up/Down,
Home/End and the mouse wheel browse commands. A click selects; Enter runs the selection.
Typing after `/` filters the list, and Esc restores a draft opened through F4.

Validation:

- TUI unit suite: **122 passed**, 1 manual benchmark ignored.
- Three new regressions cover a 120×45 window, a 40×10 window, resize, last-command
  visibility, page navigation, wheel/click row mapping, non-actionable borders/footers,
  stale filters, and preserving a selected multiline draft.
- Clippy for TUI/CLI, including all targets: passed with warnings denied.
- Repository fast gate: passed.
- Independent subagent read-only review found no blocking issue.

These menu checks use Ratatui's TestBackend. The initial environment refused `/dev/tty`;
no live screenshot is claimed. The later [full-gate retry](precommit-validation-2026-10-01.md)
also exercises the workspace in a real pseudo-terminal against a scripted frontier.

Release build: `duet 0.1.0`, `target/release/duet`.

SHA-256: `d36d551f6e27128c9b7e762076aee7ea1fe99269219e185ee964a4b270483203`.
