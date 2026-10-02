# TUI interaction validation — 2026-10-01

Platform: macOS, Apple Silicon. Working-tree build; this report covers the editor,
clipboard, search and attachment changes, not a complete product certification.

## Automated checks

| Check | Result | Scope |
|---|---:|---|
| `cargo test --offline --jobs 2 -p duet-tui -p duet-cli --lib` | 119 TUI + 77 CLI passed; 1 manual benchmark ignored | Editor, rendered UI, clipboard adapters, settings paste, goals/history, production chat loop with mock frontier |
| `cargo test --offline --jobs 2 -p duet-agent --lib images::tests` | 6 passed | Original-path privacy, configured root aliases, symlink refusal, bounded image reads and resume-cache validation |
| `cargo clippy --offline --jobs 2 -p duet-tui -p duet-cli -p duet-agent --all-targets -- -D warnings` | Passed | Includes integration/example/test target compilation |
| `bash tools/gate.sh --fast` | Passed | Format, licenses, construction constraints, provenance |
| `git diff --check` | Passed | Whitespace/conflict checks |
| Release build | Passed | `cargo build --offline --release --jobs 2 -p duet-cli --bin duet` |

Cargo ran with `CARGO_INCREMENTAL=0`. No paid model calls were used.

## Built artifact

`target/release/duet` (`duet 0.1.0`)

SHA-256: `27310f5db31d8fbd18c83cf831f0f79e53248dd2bcf008949871114718e4fc0d`

The final release binary passed a line-mode smoke test for relative workspace attachments,
listing IDs, detaching, an empty queue and clean exit without creating a model session.
[Machine-readable result](tui-cli-smoke-2026-10-01.json).

## Behaviors checked

- Shift navigation, Ctrl-A, copy/cut routing, atomic multiline paste, undo/redo,
  grapheme boundaries (emoji, CJK, combining marks), and mouse coordinates through wrapping.
- Input selection remains visible without color. Selecting conversation text alone does not
  change the clipboard; explicit copy cannot interrupt a turn.
- Pasted text cannot submit a message or answer an approval. Settings paste cannot confirm a change.
- Asynchronous clipboard results cannot overwrite a newer draft. Enter waits for an image queue job.
- Find and command palette preserve the draft. Attachment failure recovery preserves newly typed input.
- Native clipboard helper caps/deadlines and text/image parsing through mock helpers. SSH clipboard
  reads and remote X11 reads are refused. Copy reports failure rather than assuming success.
- CLI production chat loop tests associate images A/B with their intended messages; failed image
  groups retain their unsent request and cannot contaminate later requests. Background image commands
  cannot satisfy an approval answer, including when the approval sequence starts at zero.
- Relative attachment paths use the selected workspace even if the shell directory contains a
  file with the same name.
- Private clipboard snapshots are external to the workspace, validated, permission-restricted,
  never automatically marked public, and removed on normal owner drop.

## Native and interactive verification limits

macOS JXA helper scripts were syntax-checked without executing clipboard reads/writes.
The TIFF conversion helper successfully converted a generated 2×1 TIFF to a valid PNG using
AppKit. The actual clipboard was neither read nor changed. Linux clipboard behavior is covered
by fake helpers; no live Wayland or X11 session was available.

A release-binary interactive smoke test was attempted in a real 120×38 pseudo-terminal. Its stdin/stdout were
TTYs, but the initial environment denied opening `/dev/tty` with `EPERM`, so Duet fell back to line mode.
A direct diagnostic confirmed the denial. Full interactive terminal/OS clipboard verification
was therefore incomplete. Computer-use capture was unavailable (`CUA_REPL_ENABLED_SURFACES is required`).
No new screenshot is presented as evidence of a live TUI. UI assertions use Ratatui's TestBackend.

## Manual check on a normal terminal

1. Run `./target/release/duet` in a disposable repository.
2. Paste multiline text, select with Shift-arrows or the mouse, copy/cut, undo with Ctrl-Z and
   redo with Ctrl-Y. Pasting must leave the message unsent.
3. Copy a screenshot, use Ctrl-V (or `/paste`), wait for the attachment chip, then inspect
   `/attachments` and remove it with `/detach i1`. Image routing still requires the configured
   vision/privacy settings; no public designation is added automatically.
4. Open Find with Ctrl-F, close with Esc; open the command palette with F4 and cancel.
   The draft and cursor should be preserved by the palette.
5. While a turn runs, queue two image/message pairs. Each image must accompany its own message;
   a failed attachment must keep its message unsent. Test approval prompts during image preparation.
6. Try the same over SSH: native image paste must explain `/image PATH`; native terminal text
   paste remains available. OSC 52 copy support depends on terminal permissions.

The complete workspace test suite and remote deployment were not run as part of this TUI gate.
See the later [commit validation](precommit-validation-2026-10-01.md) for the full-suite retry
with expanded permissions, including the existing pseudo-terminal integration test. Actual
desktop clipboard checks and a fresh live screenshot remain outstanding.
