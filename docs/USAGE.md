# Using Duet

How to run Duet, code with it in sessions, and configure its tools, models and settings. Why Duet
exists and how it keeps sensitive information local: [README](../README.md). The threat model and
every security rule in detail: [SECURITY.md](../SECURITY.md). For a shorter control
overview and evaluation procedure, see [the security guide](SECURE_BY_DESIGN.md).

[Commands](#commands) · [Goals](#goals-and-history) · [Modes](#modes-and-acceptance-checks) ·
[Workspace](#coding-with-duet-the-workspace) · [Tools and privacy](#tools-and-privacy-controls) ·
[Models](#models) · [Pricing](#costs-and-pricing) · [Configuration](#configuration)

## Commands

```sh
duet                            # the workspace: code in a conversation (hybrid mode by default)
duet "fix the failing export"   # the same, with the first message given
duet --mode top-clearance       # configured local model only; no frontier
duet --resume                   # continue the most recent open session (or --resume <id>)
duet --goal "fix and verify the export" --check 'cargo test'  # keep working within budgets
duet history                    # recent sessions and runs, with resume commands
duet history --search export    # search saved titles and conversations
duet history <id>               # read a conversation; --json for scripts
duet skills list                # discover portable SKILL.md workflows and diagnostics
duet skills show code-review    # read a skill locally, without calling a model
duet plugins inspect examples/plugins/quality-kit  # inspect a native package; runs no code
duet plugins install examples/plugins/quality-kit  # enable a private snapshot for the next session
duet plugins list               # installed packages, capabilities and content digests
duet run "fix the failing billing export"      # one-shot: work to a terminal state, no conversation
duet run --check 'cargo test --offline' "fix the export"  # require a passing check at finish
duet --check 'npm test' "build the page"  # the same contract for a session
duet run --quiet "..."          # the same without progress on standard error (the summary is unchanged)
duet run --image shot.png "fix this layout bug"  # attach an image (repeatable; --image-public: see Images)
duet audit show <run>           # see exactly what was sent to the frontier, and the security events
duet audit verify <run>         # check the hash chain and its anchor
duet audit export <run>         # verified metadata-only JSON; no request or event text
duet audit check [run]          # nonzero exit for missing, altered or unanchored logs
duet audit retention            # review raw-data expiry and audit archival thresholds
duet purge --dry-run            # preview expired raw runs without deleting them
duet privacy                    # offline file rules, model destinations and exceptions
duet audit disclosure <run>     # what was withheld from the frontier, by class (counts only)
duet resume <run>               # continue an interrupted one-shot run
duet config list                # every setting, its value and where it came from
duet config set --project ip.interface_only '["src/pricing/**"]'
duet setup                      # discover credentials and served models; review and save settings
duet setup --provider openai --yes   # explicit cloud recipient; apply without a prompt
duet setup --local-url http://127.0.0.1:9000/v1  # custom local server
duet config preset              # show all local and frontier presets
duet config preset ollama --model qwen3:8b --confirm   # point the local role at one (audited)
duet config preset anthropic --confirm                 # frontier endpoint, model, key variable, dialect
duet doctor                     # pass/warn/fail with a fix per check; no network (--online, --json)
duet local-eval                 # measure the configured local model in its reading roles
duet scan --rules-only          # scan existing code without model opinions
duet scan --background          # return a scan id; inspect with duet scan --status <id>
duet purge                      # delete raw run data older than the retention period
```

[Privacy preview](OPERATIONS.md#preview-privacy-before-starting) explains the offline report and its exit codes.
[Operations](OPERATIONS.md) covers audit export, monitoring and retention.

## Plan before implementing

Use `/plan` or `/plan on` to inspect the workspace and discuss an approach with
read-only tools. `/plan <task>` enters planning and sends the task in one step:

```text
/plan Review the billing export and propose a migration
/plan status
/plan off
Implement the migration we discussed, then run the tests.
```

Planning works before the first message and survives `duet --resume`. During
active work, the switch waits for the current step to finish and holds subsequent
input for the new mode. `/plan off` leaves planning without executing the plan;
send an implementation request when ready. Ordinary messages never exit planning.

The `PLAN` indicator remains visible in the workspace and `/status`. Duet enforces
an allowlist of planning tools: repository reads and searches, local analysis,
and conversation tools. It blocks edits, shell commands, undo, browser/web tools,
MCP/LSP services and delegated agents while planning. Session and audit records
are still written locally, and model calls still follow the selected privacy mode
and consume its budgets. Planning pauses automatic goals; `/goal` start and resume
are refused until you leave planning. Leaving does not resume a paused goal:
use `/goal resume` separately if that is what you want.

## Goals and history

Start an ongoing goal with `duet --goal "what you want accomplished"`, or type
`/goal <objective>` in the workspace. Duet continues after progress replies without needing
another “go”. It stops for a question, failure, stop request, or budget limit. Completion means
the agent called `finish` and every configured acceptance check passed; choose meaningful
checks with `--check` or `checks.commands` for the quality you need.

| In the workspace | What it does |
|---|---|
| `/goal` | Show the objective, state, progress and remaining turns |
| `/goal pause` or `/stop` | Pause after the current step; Ctrl-C interrupts immediately |
| `/goal resume` | Explicitly continue a paused goal using its remaining allowance |
| A reply to Duet's question | Answer and continue the waiting goal |
| `/goal cancel` | End the goal without claiming success; a new goal can then start |
| `/history` | List recent work with its status, cost and resume command |
| `/history <id>` | Read a saved conversation without calling a model |

Goals allow **20 turns by default**, configurable for new goals with `--goal-turns N`
(1–1000). Each turn also obeys the existing request, time and dollar limits. Cumulative
`session.frontier_usd` and `session.wall_clock_minutes` limits still apply across goals and
process restarts. Reserving a goal turn happens before its work starts, so interruption does
not refund that turn. A used-up allowance cannot be reset by resuming.

`/quit` leaves the session resumable and pauses active goal work. After reopening it with
`duet --resume <id>`, inspect `/goal` and explicitly use `/goal resume`. Reopening never starts
goal requests by itself. `/close` closes the session and cancels any unfinished goal. A crash
also recovers to a paused goal. Duet must remain running for automatic work to continue.

History is local to the workspace. `duet history --search TEXT --limit 50` searches saved
objectives and messages; `--json` provides structured records. Large histories are read with
limits, and partial records are labelled. History browsing never repairs or truncates a log
that may still be receiving writes. Goal state and progress are private files beside the
session transcript in `.duet/runs/<id>/`; `duet purge` retention applies to both. Goals enter
the same privacy boundary as ordinary messages.

## Modes and acceptance checks

`duet run --mode passthrough --no-privacy` runs the frontier alone with the boundary off (the
evaluation baseline). A repository can forbid it, for new and resumed runs and sessions alike:
`duet config set --project frontier.allow_passthrough false` (turning it back on is the owner's,
with `--confirm`).

### Acceptance checks

**Acceptance checks.** Repeat `--check COMMAND` to add task-specific checks to the project's
`checks.commands`. Duet runs them in its command sandbox when the agent calls `finish`; a failing
check returns its filtered output to the agent for repair, and the task cannot finish while a check
fails. Checks are saved with the run or session, so resume uses the same commands even if project
settings change. Use executable tests of the requested behavior: for a browser game, a project
script can check item counts, progression and victory conditions and exercise the first screen in a
headless browser. A parse check alone does not establish that the game meets its brief. Passing
checks use local command time and the run's finish-attempt budget without a separate model reviewer;
failures can add frontier repair turns and cost.

### Top clearance

**Top clearance.** For work whose content may not reach a frontier provider, `--mode
top-clearance` (in a session, `/mode top-clearance`) has the local model do everything: it reads,
decides and writes the code. Requests go to the configured local endpoint, which may be a
self-hosted server. Use an approved loopback endpoint for same-device inference; a remote endpoint
adds that server and its transport to the trusted environment.
There is no frontier; the web tools are not offered; commands get no network whatever
`sandbox.network` says (no egress proxy, no package registries: dependencies must already be on
disk); MCP servers reached over HTTP or with `network = true` are not started; sub-agents use the
local model. The header shows TOP CLEARANCE, and the audit log records every request to the local
model, so what left can be checked (`duet audit show`). The work is only as good as the local
model. Evaluate the configured model on your tasks. In a session, `/mode top-clearance` leaves the
current session open (`duet --resume` continues it) and starts a new one in top clearance, fresh:
nothing said in it ever reaches the frontier. It cannot be left again within that session, because
its conversation holds what only the local model may see; `/close` it and start another session.
`/mode` alone shows the session's mode. A repository can require it for every run and session:
`duet config set --project clearance.required '"top"'`; `duet` and `duet run` then start in top
clearance without `--mode`, and every other mode is refused (lifting it is the owner's, confirmed).
`--mode local-only` is the same mode under its old name. Details: [SECURITY.md](../SECURITY.md)
(Top clearance).

## Repository instructions and extensions

**Project instructions.** Duet reads `AGENTS.md` (or `AGENTS.override.md`), `CLAUDE.md`,
`GEMINI.md`, root `.github/copilot-instructions.md`, then `DUET.md`. More specific directories
take precedence. The current user request outranks owner instructions, which outrank
repository guidance; all remain within Duet's host rules. Owner equivalents beside
`~/.config/duet/config.toml` are loaded first;
`DUET_CONFIG_HOME` changes that directory. Root instructions enter the opening message once
and are replayed on resume. Relevant descendant instructions load for the path named by
`read_file`, `edit_file`, `write_file`, `edit_protected` or `rename`; a newly discovered block
defers that operation until the model has seen it. The model is instructed to read files before
shell changes; shell paths and additional rename targets are not automatically enumerated.
Each file contributes at
most 16 KiB, combined root instructions at most 64 KiB and scoped blocks at most 32 KiB.
Project text passes the privacy boundary and cannot change policy or the sandbox.
[Ordering, scope, refresh and limits](EXTENSIONS.md#repository-instructions).

**Skills and plugins.** Put a workflow in `.agents/skills/<name>/SKILL.md`, with YAML
`name` and `description` fields followed by Markdown instructions. Duet also discovers familiar
owner and project skill locations. It sends short metadata first and loads full instructions
and referenced text only when needed. Install a native package with `duet plugins install PATH`;
inspect it first with `duet plugins inspect PATH`. Installation takes a private content snapshot
and executes no scripts. Enabled packages can contribute MCP servers at the next session start.

| In the workspace | What it does |
|---|---|
| `/skills` | List skills and discovery diagnostics |
| `/skill <name> [task]` | Select a workflow, including one requiring explicit invocation |
| `/plugins` | Show installed packages and their capabilities |
| `/command plugin:name [arguments]` | Use a packaged Markdown prompt command |

Plugin skill names are qualified, for example `quality-kit:code-review`. Use `duet skills show
quality-kit:code-review` to inspect one locally. Restart the session after installing or updating
extensions to refresh its catalog and tool servers. Repository settings can disable discovery
with `extensions.skills_enabled = false` or packages with `extensions.plugins_enabled = false`.
Skill instructions grant no tools or permissions. [Example, lifecycle, privacy and compatibility](EXTENSIONS.md).

**Without git.** Duet works in any folder. Outside a git repository, files are listed and searched
by a walk that honours `.gitignore` and `.ignore` files and skips `.git`, `.duet` and dependency and
build-output directories (`node_modules`, `target`, `dist`, `__pycache__`, `.venv`, ...); `diff`
and `/diff` compare the files duet wrote with their content before the run (changes made only by
commands are not shown); `/undo`, resume and the audit work as in a repository; the git tools are
not offered (`git init` adds them).

## Security review and repository scans

```sh
duet scan                          # existing code, rules and eligible local opinions
duet scan --rules-only --json       # no model or language-server requests
duet scan --fail-on-high            # exit 2 for rule-confirmed high findings
duet scan --background              # detached worker; prints the scan id
duet scan --status scan-...         # state, commit, dirty flag and private report location
duet config set review.enabled true
duet config set review.block_high true  # optional finish enforcement; requires review.enabled
```

Scanning does not require `review.enabled`; that setting adds review to `finish` after ordinary
checks pass. Findings compare against a private run-start snapshot, including pre-existing dirty
files. Resume retains that baseline. A rule-confirmed high finding can enter the normal repair
loop when enforcement is enabled. Model opinions cannot create a blocker or hide a finding.

Reports live at `.duet/runs/<id>/security-review.json`. Repository scans also write
`scan-status.json`, recording the starting commit (or null outside git), dirty state, timing and
model usage. `scan-usage.json` preserves metering even if a started review fails. A captured
source or commit changing during review prevents completion. Incomplete
coverage is explicit; an empty report is not a security certificate. A background process killed
without cleanup can leave a running status; inspect its PID and start a new scan.

Local review defaults to protected code and privacy flows. `review.local_open = true` also asks
the local model about ordinary open-code candidates; measurements have not shown an accuracy
gain there. `review.max_candidates` limits local opinions (default 16). Configured sandboxed
language servers supply bounded context when `review.references` is true. They are not installed
automatically. Pass-through and top-clearance finish auditing currently use rules and installed
scanners without a separate local reviewer; a manual scan uses the configured local reader.

An optional fresh-context frontier opinion has no working conversation or tools. It is offered
only for high or locally uncertain findings on eligible open code. Protected paths, private
values, recognized privacy flows and external-scanner candidates are excluded. Enable it with
`duet config set review.frontier true --confirm` (owner only). The extra send requires the
configuration command's existing privacy-loosening confirmation. Its default limits are four
opinions per attempt and $0.50 per reviewer, also constrained by the remaining run budget.
Usage, failed attempts and dollars survive resume. Top clearance never uses this frontier role.

Optional scanners must already be installed outside the repository. Configure their absolute
executable, argument array, output format and timeout in owner configuration:

```toml
[review.scanners.example]
command = "/absolute/path/to/installed/scanner"
args = ["--format", "sarif", "."] # replace with that scanner's actual arguments
format = "sarif"
timeout_seconds = 60
```

Formats: SARIF, Bandit, gosec, cargo-audit, npm-audit and pip-audit JSON. At most four tools run,
with no network or source writes, on a bounded UTF-8 snapshot. `{workspace}` in an argument is
replaced with that snapshot's directory. Offline databases and configuration must already be
available; tools requiring network access report unavailable. `--rules-only` still runs these
configured tools. Duet bundles no third-party scanner rules and does not execute reviewed source.
Scanner failures and explicit analysis errors mark coverage incomplete. Exit 1 is accepted
only with validated findings; other nonzero exits are failures. New cross-file findings are
retained even when the scanner reports them in an unchanged file.
Raw tool JSON is private at `security-scanners/<index>-{before,after}.json`; untrusted descriptions
do not enter model prompts. Only locations inside the snapshot become advisory findings.

Defaults remain off for finish review, blocking and frontier opinions. The
[auditor report](evidence/reviews/security-auditor-2026-09-30.md) records coverage, measurements and known limits.

## Coding with duet: the workspace

`duet` opens the workspace: one session (one conversation in one workspace) in full screen. You
give a task, duet works on it with its tools (each step shows as a progress line), and the turn ends
with duet's reply, a question for you, or a finished task; your next message continues with
everything said and done so far.

```text
  DUET  /  billing                        F1 help · Ctrl-O details · Tab panel · F2 settings
  hybrid · frontier glm-5.3-flash · local omlx-coding · privacy boundary active
                                                        │  Changes  Privacy  Session   Tab / ⇧Tab
  › you                                                 │ 1 file(s)  +3 −1
  │ The CSV export drops the last row. Fix it.          │ › ● src/export.rs  +3 −1
                                                        │ ──────────────────────────────────
  ● read  src/export.rs                                 │ 41 - for i in 0..rows.len() - 1 {
    │ src/export.rs (lines 1-120 of 120)                │ 41 + for i in 0..rows.len() {
    … 118 more lines · Ctrl-O                           │
  ● read  data/customers.csv · done  ◦ sensitive: raw content held; filtered view prepared
                                                        │
  ● edit  src/export.rs  ◦ +1 −1                        │
    - for i in 0..rows.len() - 1 {                      │
    + for i in 0..rows.len() {                          │
  ✓ done                                                │
    The loop stopped one row early; it reads to the end, with a test.
╭ Message duet ──────────────────────────────────────────────────────────────────────────────╮
│ Describe a task or ask a question · @ names a file · / for commands…                        │
╰ Enter send · Alt-Enter new line · @ file · / commands ─────────────────────────────────────╯
 DONE  │ turn 1 · $0.0184 of $20.00 · 42.1k in · 1.2k out                              F1 help
```

- **The screen.** The header: the workspace, the mode, the frontier and local models, and whether
  the privacy boundary is active (in passthrough, a red warning says it is off). The conversation,
  as cells: your messages (`› you`), duet's replies as the frontier writes them (`◆ duet`, formatted
  lightly: headings, lists, quotes, code blocks, inline code and bold), each tool call (`● read`,
  `● run`, `✗` when it failed) with an explicit running/done/failed/stopped status and its prepared
  result (placeholders kept), folded to its first lines (Ctrl-O expands; a failure is shown longer).
  Local questions show the handle, source path when available, every question and the filtered
  answer, with a redaction count. Reads show requested line ranges; long commands remain visible
  in full. The journal also shows every
  edit with its diff (a sensitive file is named, never shown), what the boundary withheld (`◦`), and
  how each turn ended (`✓ done`, a question, a stop). While duet works a line under the conversation
  shows the turn's time, its cost so far and what runs now. It follows new output; PgUp/PgDn or the
  mouse wheel scroll back (it stays put and shows how many rows are below; Ctrl-End on empty input returns).
  The status line's badge says READY, WORKING, DONE, YOUR ANSWER, STOPPED or FAILED, beside the
  turn, the cost against the session budget and the tokens. The side panel (Tab / Shift-Tab cycle
  its tabs; Ctrl-T also cycles through closed; it opens by itself on a window 110 columns or wider):
  **Changes** lists the files the session changed with added/removed line counts above the selected
  file's diff (Ctrl-↑ ↓ pick a file, Ctrl-PgUp/PgDn scroll the diff); files that are sensitive, or
  were produced by a command that read sensitive data, are named and never shown. **Privacy** shows
  individual reads, local questions, commands and filtering decisions, newest first. Its default
  view explains what Duet did, whether a matching cloud send passed its checks, and how the result
  was handled. Ctrl-↑/↓ selects an action; Ctrl-O opens or closes its full record, including the
  exact outbound text, audit number, time and model. Ctrl-PgUp/PgDn scrolls the selected view.
  Prepared results are distinct from records that passed the outbound checks; those records do
  not confirm provider receipt. Failed and pending calls are labeled separately. Outbound
  filtering records identify the history item/call, tool, file and argument field, changed line
  numbers, replacement placeholders and where each value was first detected. They do not log
  the matched secret. The journal groups repeated descriptions from history rechecks; Privacy
  retains the details for each request. Older count-only records are labeled incomplete because
  they did not record the affected fields. Resuming reloads the recorded privacy events.
  Local-model endpoint decisions show the host and trust decision. The latest 500 activities are retained
  in this view; `/audit` opens the full record. Metadata and filtered views are shown without
  opening raw handles or restoring their secret values. **Session** shows turns, requests,
  tool calls, tokens, frontier and local cost estimates, the model/rates/catalog source used,
  and working time against the budgets. Ctrl-PgUp/PgDn scrolls Session details too.
  Colour unless `NO_COLOR` is set.
- **Talking to duet.** Every message is a turn. duet ends it with `duet:` (a reply), `duet asks:`
  (a clarifying question: your next message is the answer) or `duet finished:` (it called `finish`
  and `checks.commands` passed). `//text` sends a message that starts with `/`. `@path` names a
  file of the workspace (Tab completes it). The input box stays editable the whole time:

  | Keys | |
  |---|---|
  | Enter | send (a line ending with `\` continues instead) |
  | Alt-Enter, Shift-Enter, Ctrl-J | a new line in the message (Shift-Enter where the terminal reports it: kitty, WezTerm, Ghostty, foot, recent iTerm2) |
  | ← →, Home / End, Ctrl-Home / Ctrl-End | move by grapheme, line or message; Alt-B / Alt-F and Ctrl-← / Ctrl-→ move by word |
  | Shift + arrows / Home / End | select text; add Ctrl or Alt for word selection |
  | Ctrl-A | select the whole message |
  | Ctrl-C / Ctrl-X | copy / cut selected input; Ctrl-C copies a conversation selection before considering interruption |
  | Ctrl-V, Shift-Insert, `/paste` | explicitly read the local clipboard: insert text or queue an image for the next message |
  | Ctrl-Insert / Shift-Delete | copy / cut selected input |
  | Ctrl-Z / Ctrl-Y, Ctrl-Shift-Z | undo / redo message edits; each paste is one edit |
  | Backspace, Delete, Ctrl-W, Alt-Backspace, Alt-D, Ctrl-K, Ctrl-U, Alt-Y | delete; cut a word or to line end/start; restore the last internal cut |
  | ↑ ↓, Ctrl-N | move between lines, then through earlier messages in this session |
  | Ctrl-R | search message history (type to narrow, Ctrl-R for an older match, Enter takes it into input, Esc cancels) |
  | `/`, F4, Ctrl-Shift-P | command palette; type to filter, ↑↓ / wheel selects, PgUp/PgDn pages, Home/End jumps to first/last, Enter runs, Tab inserts; F4 preserves the draft and Esc restores it |
  | `@`, Ctrl-P | choose a workspace file reference; Ctrl-P appends a picker to the draft |
  | Ctrl-F, `/find` | search rendered conversation text; Enter / Shift-Enter moves between matching rows; Esc returns to the draft |
  | F3 / Shift-F3 | next / previous search result after closing Find |
  | Ctrl-Shift-C, `/copy` | copy the latest reply when nothing is selected |
  | Tab / Shift-Tab | next / previous panel; Tab accepts an active completion or completes `/image` and `/attach` paths |
  | PgUp / PgDn, mouse wheel | scroll conversation; the wheel over panel details scrolls that panel |
  | Ctrl-End with empty input | return to the latest conversation output |
  | Ctrl-O | unfold tool results; in Privacy, show summary / full record |
  | Ctrl-T | cycle Changes, Privacy, Session, closed |
  | Ctrl-↑ / Ctrl-↓, Ctrl-PgUp / Ctrl-PgDn | select a changed file or privacy event; scroll its details |
  | F2 / F1 | settings / scrollable shortcut help |
  | Mouse click / drag | position the input cursor / select text in the input or conversation; Ctrl-C copies |
  | Ctrl-Alt-Z / Ctrl-D | suspend (`fg` resumes) / leave on empty input |

  The command menu uses the available terminal height. When commands do not all fit, it shows
  the visible range, how many remain above/below, and a scrollbar. Click a command to select it;
  Enter runs it. Mouse scrolling over the menu browses commands; outside it, scrolling remains
  with the conversation or side panel. Help, goals, history, status, models and settings come first.

  Text selection supports combining marks, CJK and emoji sequences. Pasted text never submits
  itself. The message editor holds up to 256 KiB and keeps bounded undo history. Ctrl-C with no
  selection clears the draft and follows the interruption rules below. To use the terminal's own
  selection instead, use its mouse modifier (often Option on macOS or Shift on Linux).

  **Clipboard images.** On macOS, copy an image or screenshot, then press **Ctrl-V** or type
  **`/paste`**. Native **Cmd-V** is handled by the terminal and ordinarily pastes text; Duet cannot
  force a terminal to forward an intercepted shortcut. Linux image paste uses system-installed
  `wl-clipboard` on Wayland or `xclip` on X11; `xsel` supports text only. Clipboard actions run in
  a worker with bounded input/output and a three-second helper deadline. There is no clipboard
  polling or clipboard read through OSC 52. Under SSH, use terminal text paste or `/image PATH`
  for an image already on the remote host. Copy can fall back to an OSC 52 write request; the
  terminal decides whether to accept it, and Duet reports that as a request rather than a confirmed
  clipboard change. Clipboard behavior on Linux is covered by mock helpers, not a live desktop test.

  Pasted images are validated and re-encoded as PNG, stored in a private temporary directory
  outside the repository, and removed when the chat ends normally. They are **not marked public**:
  existing image privacy and model-vision settings determine their destination. Clipboard input
  is limited to 16 MiB encoded, 40 million pixels and 16,384 pixels per side; normal image preparation
  applies its additional limits. The temporary store allows up to 64 captures / 256 MiB per chat.
  A crash may leave private temporary files; run-image copies follow normal run retention.

  Queued attachments appear above the message. **`/attachments`** lists their current IDs;
  **`/detach i1`**, **`/detach t1`**, or **`/detach all`** removes them from the queue without deleting
  their source files. Dropped PNG/JPEG/GIF/WebP paths are recognized as images, including quoted
  paths with spaces. Relative attachment paths use the displayed workspace, including with
  `--workspace`. Enter waits while a clipboard image is being prepared. An image queued
  during a running turn is paired with the following message after that turn. If it fails to
  attach, its message is kept unsent for review. File references chosen with `@` are references
  in the prompt; `/attach` snapshots file content.

  Find searches rendered rows (unfold tool details with Ctrl-O to include their contents).
  Settings fields accept bracketed text paste without submitting or approving a change.
  Warnings and setup output appear in the conversation, never over the input.
- **Steering while it works.** Type while duet is working: the message is delivered after the
  current step (its tool results are recorded first; a running command is never cut short), and
  duet takes it into account from its next step. Several messages typed meanwhile arrive together,
  in order. `/stop` ends the turn after the current step; **Ctrl-C** ends it at once and kills a
  running command. Either way the session stays open and duet is told what happened.
- **Commands** (never sent to the model): `/status` (turns, tokens, cost and working time against
  the budgets), `/diff` (the workspace against the last commit, or outside a repository the files
  duet wrote against their earlier content; sensitive files are named, not shown), `/undo` (reverts
  the files the last turn wrote through its tools; repeat to go back further; changes made by
  commands are not reverted), `/image <path>` and `/image --public <path>` (attach an image to your
  next message; see Images below), `/mode` (the session's mode) and `/mode top-clearance`
  (continue in top clearance, above), `/quit` (leave; the session stays open), `/close` (end it for
  good), `/help`; and the settings screens: `/settings`, `/models`, `/sensitivity`, `/ip`,
  `/limits`, `/data`, `/audit`, `/runs`.
- **Resuming.** `/quit`, Ctrl-D or Ctrl-C twice at the prompt leaves the session open; `duet
  --resume` continues the latest open one (with a recap of its last turns), `--resume <id>` a given
  one, also after a crash. A normal exit records `open` in the summary and audit end event.
  History also recognizes older normal exits without rewriting their saved logs; actual failures
  and interrupted turns retain their own status.
- **Text attachments.** Drag a Markdown or other UTF-8 text file into the input, or enter
  `/attach /path/to/spec.md`, then send your instruction. Duet confirms the filename and size;
  quoted paths and shell-escaped spaces/parentheses work. The file is snapshotted when attached,
  including files outside the workspace, and included with the next message or steering message.
  Sensitive files use local handles; detected private values are filtered. Large files use the
  normal bounded file view. Each file is limited to 512 KiB, with at most 16 files / 2 MiB pending.
  Attachments delivered in a session remain in its private transcript/handles when resumed.
- **Privacy.** You see real values in your terminal; the frontier gets your messages the way it
  gets task text: detected secrets and personal data become placeholders, and values it has seen
  as placeholders stay placeholders. duet's replies show the real values back to you. See
  [SECURITY.md](../SECURITY.md) (Operator messages).
- **Limits.** Each turn is held to `limits.frontier_usd` and `limits.wall_clock_minutes`; the
  session to `session.frontier_usd` and `session.wall_clock_minutes` (time duet works; time
  waiting for you does not count). A turn stopped by a limit leaves the session open; a spent
  session budget ends it until the budget is raised. `oversight.approve` and commits ask in a
  dialog (No is the default; ↑↓ or y/n, Enter answers, Esc denies); with approval on, a session
  needs a terminal.
- **Memory.** Every command, MCP server and language server Duet starts is held to
  `limits.process_memory_mb` per process and `limits.command_memory_mb` for all of a command's
  processes together (defaults: an eighth and a quarter of this machine's memory, counting
  compressed and swapped memory too). A process above them is stopped and the command's output
  says which and why; when the whole machine is critically short of memory, the command's largest
  process is stopped. Duet never stops a process it did not start.
- **Commits.** In a git repository, a session at a terminal offers `git_commit` with the defaults
  (`git.commit = "ask"`, approval off): before each commit it shows the files and the message and
  waits for your `y`. Nothing else is asked unless `oversight.approve` is on.
- **Without a terminal** (a pipe, a script) `duet` is the same session line by line: your lines
  are read from standard input and duet's output is plain lines.

`duet run` stays the one-shot path (scripts, evaluation): no conversation, and its requests are
unchanged. Its progress goes to standard error so it never looks stalled: on a terminal the steps,
the frontier's text as it streams and a status line (time, cost, output received so far, what
runs now); otherwise one plain line per step with placeholders kept as the frontier saw them, and
a line every 30 seconds while nothing else happens. Standard output holds only the summary, as
before; `--quiet` turns the progress off.

### Workspace settings

**Settings, inside the workspace.** F2 or `/settings` (or `/models`, `/sensitivity`, `/ip`,
`/limits`, `/data`, `/audit`, `/runs` for one screen) opens seven screens over the conversation;
Esc (or `q`) comes back. Models (frontier and local settings, with `duet doctor` offline; `o` adds
the online checks of connection and context window; `l` looks for local servers on loopback (the
preset ports, as bootstrap does) and `[` `]` `u` point the local role at one; `c` runs the
cache-reuse probe, two identical short requests to the local model, only when pressed), Sensitivity
(globs, detectors, custom detector patterns, raw-output commands, secret sinks, bulky thresholds;
`t` tests a path — sensitive or not and which pattern matched — and `s` tests sample text, showing
what the detectors and your patterns would replace), IP levels (the workspace tree as git sees it,
so `.gitignore` applies; `i` / `s` mark a file or directory interface-only / sealed, with a preview
of the skeleton the frontier would see), Limits, Data (retention; `x` purges the raw data of runs
older than `data.retention_days`, `X` of every run, after listing them and asking; audit logs are
kept), Audit (each run's records and outbound request summaries as stored, and `v` to verify the
hash chain and its anchor) and Runs (the workspace's runs and sessions, read-only: each as it was
written, beside the files it changed and their diffs; `[` `]` pick one, `←` `→` switch panels,
`J` `K` scroll the diff). The settings screens are generated from the registry and show where each
value comes from (default, owner or project). Edits take the same path as `duet config set`: a
change that loosens privacy shows its diff and needs `y`, `p` switches edits to the project file
(which only tightens and never takes owner-only keys), and every applied change is recorded in the
owner's config audit log. Changes apply from the next session.

## Tools and privacy controls

### Sensitive-content detection

**What the detectors find.** Duet's own secret and personal-data detectors, the gitleaks rule set
(221 rules for specific services' credentials, used as data: pinned in
`crates/duet-boundary/rules/`, updated with `tools/update-rules.sh <version>`), international
phone numbers, IBANs, the main EU/UK national IDs with their check digits, IPv6 and labelled postal
addresses. `duet doctor` shows the rule set in use; [SECURITY.md](../SECURITY.md) (Detection) lists
everything with measured recall, false positives and limits. A person's name in free text has no
shape to detect: with `sensitivity.local_pii_pass = true` (off by default; it costs local model
time on every public result with prose) the local model marks names and postal addresses in the
prose of public content, and they become placeholders like detected values.

**Custom detector patterns.** `sensitivity.custom_patterns` takes regular expressions for values
only you know are sensitive (customer ids, internal host names): every match becomes a `data`
placeholder, in sensitive and public text alike. A project may add patterns; removing one loosens
privacy and needs confirmation.

### Operator approval

**Operator approval** (`oversight.approve`, owner config only; default `off`): with `risky`, Duet
asks y/N on the terminal before a `sensitive_data` command, a protected edit, or a write to anything
other than an ordinary source or test file; with `all`, before every command and write. A refusal
is returned to the model as a tool error, and every decision is in the run's audit log. A run with
approval on and no terminal refuses to start. Details: [SECURITY.md](../SECURITY.md) (Oversight).

### Command networking and troubleshooting

**Network for commands** (`sandbox.network`; default `registries`): commands reach the package
registries in `sandbox.registries` through Duet's egress proxy, so `npm install`, `cargo add`,
`pip install`, `go get` and the like work; the defaults are crates.io, npm (and yarn's mirror),
PyPI, the Go module proxy and checksum database, Maven Central, the Gradle plugin portal, RubyGems
and GitHub's download hosts (release assets and source archives; not github.com, which also takes
pushes). Any other host is refused, and the command's output ends with what was refused
(`[sandbox] the egress proxy refused: example.com:443 ...`). A server a command starts on
localhost is reachable from the same command (under bubblewrap on any port; on macOS on the usual
development ports, 3000-3099, 4000-4099, 5000-5099, 5173-5199, 7000-7099, 8000-8099, 9000-9099
and a few others, when nothing on your machine already listens there, so a test server on a
random port needs `all` there). `"off"` gives commands no network; `"all"` gives them all of it
(`duet config set sandbox.network '"all"' --confirm`). Whatever the mode, a `sensitive_data`
command has no network, nor does a check that can read protected source. Add a registry (owner
config only; adding asks for `--confirm`, removing does not):
`duet config set sandbox.registries '["registry.npmjs.org", "npm.pkg.github.com"]' --confirm`
(host names, `*.domain`, optionally `:port`; without a port 443 and 80). A registry on a private
address (an intranet mirror) is refused; use `all` for it. Package caches are kept per run in its
scratch directory (npm, pip and the others download again in a new run; cargo reuses the crates
your own `~/.cargo` holds, which commands can read but never write). Commands cannot read the
credential stores in your home directory (`~/.npmrc`, `~/.cargo/credentials.toml`, `~/.ssh`,
`~/.aws`, shell histories and startup files, browser profiles; the list is `HOME_SECRETS` in
`crates/duet-sandbox`), so a token in `~/.npmrc` is not available to them. Each connection is an
`egress` event in the run's audit log (host, port, bytes each way, allowed or refused; never a
path). `cargo new` makes no `.git` in commands (they may not create one). An owner config from an
earlier version with `sandbox.network = true` or `false` still works: it reads as `"all"` or
`"off"` and Duet prints a note. Details: [SECURITY.md](../SECURITY.md) (Command network).

**Command troubleshooting.** Temporary files belong in the command's private `$TMPDIR` or the
workspace. A hard-coded `/tmp/file` or home-directory write can be denied. Use `list_files`,
`search` and `read_file` for file discovery and reads; commands cannot inspect `.duet` or `.git`.
Skills cannot grant access to a browser daemon, its saved profile or host sockets.

Start a preview server and run its client check inside the **same command**. Duet stops all
child processes when that command ends, including background processes started with `nohup`.
On macOS with the default network policy, choose an unused loopback port such as 8000 or 5173;
an arbitrary port may be blocked. Keep an occupied host service running and choose another port.
Connection errors and permission denials now include recovery guidance without changing policy.
Check the actual test's exit status: a trailing `cat`, `head` or `tail` can hide a pipeline failure.

**Visual verification.** Screenshots and visual claims require the actual application or browser
renderer. A canvas mock can test logic, but its output does not prove that the real interface
renders correctly. If the configured tools cannot launch a browser or inspect an image, Duet
should report that limit. A refused image read creates no content handle: `ask_local` cannot
inspect a filename or an invented ID. Configure a local vision model, or explicitly attach a
non-sensitive image with `/image --public PATH` when the frontier supports vision. Sensitive
and protected paths keep their existing restrictions. See Images below.

### Web tools and search

**Web tools** (`web.*` settings; on by default): the frontier gets `web_fetch` (a public page
as text; HTML is converted with links kept; `start_line`/`end_line` read part of a long page) and
`web_search` (title, URL, snippet and source per result). Requests are made by the host, never by commands
(commands' own network is `sandbox.network`, below): `GET` only (a search backend's own API may `POST`),
`http`/`https`, public addresses only (checked after DNS and on every redirect), at most
`web.max_bytes` per response and `web.timeout_secs` per request. `web_fetch` does not read the
result pages of general search engines (Google, Bing, DuckDuckGo and the like): searching is
`web_search`'s job.

Search works without setup, and Duet runs it itself: with `web.search.backend = "auto"` (the
default, the `native` backend) the host asks public sources that publish an API for automated
use, in parallel, with no search provider in between, and merges their answers (each source's
first result, then each source's second, a page listed twice shown once). Every source asked
receives the query and your address, so a search now reaches several parties; `duet doctor`
("web search") lists them.

| Source | What it finds | Asked | Service and limit (without a key) |
|---|---|---|---|
| `stackoverflow` | Stack Overflow questions (answers, score, tags) | by default | Stack Exchange API; 300 requests a day per address |
| `wikipedia` | English Wikipedia articles | by default | MediaWiki API; one request at a time, one a second |
| `github` | GitHub repositories | by default | GitHub REST search, never with a token; 10 searches a minute |
| `crates`, `npm`, `pypi` | packages (`pypi`: an exact name only) | by default when the workspace root has a `Cargo.toml`, a `package.json`, or a Python project file (`pyproject.toml`, `setup.py`, `setup.cfg`, `requirements.txt`, `Pipfile`); otherwise on request | crates.io one request a second; npm's registry search; PyPI's JSON API |
| `github_issues` | GitHub issues and pull requests (error messages, bug reports) | on request | shares GitHub's 10 a minute |
| `serverfault`, `superuser`, `askubuntu`, `unix` | those Stack Exchange sites | on request | shares Stack Exchange's quota |
| `hackernews` | Hacker News stories and their discussions | on request | Algolia's HN Search API; 10,000 an hour |
| `arxiv` | arXiv papers | on request | arXiv API; one request every 3 seconds |

The frontier picks sources per search (`web_search {query, count?, sources?}`); the tool's
description lists the run's sources. Each result shows its source, and the reply starts with what
every source did (its result count, "answered earlier" for an answer kept from earlier in the run,
"skipped" when a rate limit or a source's requested pause would make it wait, "timed out"). A
source gets 10 seconds (or `web.timeout_secs` when shorter), so a slow one never holds up the
others. Stack Overflow and GitHub match every word: a few distinctive words (a name, an error
message) find more than a sentence. Google, Bing and DuckDuckGo are never asked: they publish no
API for this, and Duet does not scrape their result pages.

`web.search.sources` chooses the sources (owner only, confirmed): `["auto"]` (the default: the
table above), `auto` plus names to ask those by default too, or names alone to allow only those.

With a Z.ai frontier and its key, `auto` uses Z.ai's own search instead: its queries go only to the
provider that already receives the session, and it finds pages the native sources do not (set
`web.search.backend = "native"` to keep searches off it). Other backends are used only when the
owner names them in `web.search.backend`; a SearXNG URL or a Brave key alone does not select them
(runs and `duet doctor` name those settings as unused):

| Backend | Who receives the queries | Cost |
|---|---|---|
| `native` (`auto` without a Z.ai frontier) | each source asked (above) | none, no key |
| `searxng` | your instance at `web.search.searxng_url`, which passes queries on to the engines it is set up with | none |
| `brave` | Brave (key in `$BRAVE_API_KEY`) | Brave's plan |
| `wikipedia` | the Wikimedia Foundation (English Wikipedia's search only) | none, no key |
| `zai` (`auto` with a Z.ai frontier and its key) | Z.ai (with a Z.ai frontier, the provider that already receives the run) | coding plan: the plan's search server, counted in the plan's credits; otherwise the Web Search API, billed per search to the account balance (`web.search.zai_engine`) |

```sh
duet doctor                                                               # "web search": the backend, its sources and their hosts
duet config set web.search.sources '["auto", "github_issues"]' --confirm  # also ask GitHub's issue search by default
duet config set web.search.sources '["wikipedia", "stackoverflow"]' --confirm  # only these two
duet config preset searxng --confirm       # private search: settings for a local SearXNG + the docker command
duet config set web.search.backend '"brave"' --confirm                    # Brave Search API; key in $BRAVE_API_KEY
duet config set web.search.backend '"zai"' --confirm                      # Z.ai's search (the plan's server with a coding-plan frontier)
duet config set web.search.backend '"none"'                               # no web_search
duet config set web.allowlist_private '["wiki.corp", "10.20.0.0/16"]' --confirm   # intranet hosts for web_fetch
duet config set --project web.enabled false                              # no web tools in this repository
```

**Private search.** `duet config preset searxng --confirm` sets `web.search.backend = "searxng"` and
`web.search.searxng_url = "http://127.0.0.1:8888"`, writes a SearXNG `settings.yml` with the JSON
format enabled next to the owner config (`~/.config/duet/searxng/`, kept if it exists), and prints
the command that starts it in Docker or OrbStack, listening on loopback only:

```sh
docker run -d --name duet-searxng --restart unless-stopped \
    -p 127.0.0.1:8888:8080 \
    -v "$HOME/.config/duet/searxng:/etc/searxng" \
    docker.io/searxng/searxng:latest
duet doctor --online        # the "web search" check sends it one test query
```

Duet never starts the container itself. Queries then leave your machine only as SearXNG's own
requests to the engines it is set up with: from your address, without an account or key.

**Z.ai search** (`web.search.backend = "zai"`). With the default frontier (the GLM Coding
Plan), `zai` searches through the plan's Web Search server: no extra key, counted in the plan's
credits (1.2 credits a search as of 2026-09). Its results vary: often only a hit's site is given
(marked "the site only" in the results), and technical queries sometimes get unrelated hits. Z.ai's Web Search API
(`web.search.zai_engine = "search_pro_jina"` or `"search-prime"`) gave better results with page
addresses in tests, but it is billed per search ($0.01 as listed in 2026-09) to the account's
balance, which the coding plan does not cover; without a balance it answers that the account has
none.

In hybrid mode a URL or query holding a placeholder or a known sensitive value, in any part of the
request and in any spelling the check reads (encoded, spelled out, cut into parts, as digits of a
withheld number), is refused before its name is resolved or anything is sent (for a native search,
before any source is contacted; every request duet makes to a third party goes through one checked
client), and fetched content and
results are shown as untrusted data with your own sensitive values replaced (what a public page
holds is public: nothing on it is withheld or blocks a later request). Every call is an audit event
(host, bytes, outcome; a native search one per source asked). Native sources are held to the same
address rules as `web_fetch`. Details: [SECURITY.md](../SECURITY.md) (Web tools).

### Git tools

**Git tools** (when the workspace is a git repository; see Without git above): `git_status`, `git_log {path?, rev?,
max_count?}` (hash, date, author, subject; at most 100), `git_show {rev, path?}` (a commit's
message, files and per-file diffs, or a file as it was at `rev`) and `git_blame {path, start_line?,
end_line?}` (at most 400 lines per call). Commands still cannot read `.git`; a `git ...` command
gets a hint to use these tools. In hybrid mode history is shown by path, like `read_file`: a path
that is sensitive now is sensitive in every revision (held locally, a secret file with its values
replaced), protected source's history is withheld (sealed paths never appear), and old diffs and
commit messages are scanned, so a key removed long ago or an author's email becomes a placeholder.

`git_commit {message, paths?}` commits files the run (or session) wrote itself, or a subset of
them, to the current branch as the operator. It never commits sensitive, derived or protected
files, ignored files or files with a git filter (LFS), refuses a message holding a placeholder or a
sensitive value, runs no hook and never pushes, resets, checks out or switches branches. Each
commit is an audit event (hash, paths). `/undo` in a session reverts files, never commits.

```sh
duet config set git.author '"Ada Lovelace <ada@example.com>"'   # else user.name/user.email from git config
duet config set oversight.approve '"risky"'                     # git.commit = "ask" (default): each commit asks, in duet run too
duet config set git.commit '"allow"' --confirm                  # commit without asking (e.g. with approval off)
duet config set --project git.commit '"off"'                    # no git_commit in this repository
```

With the default `git.commit = "ask"`, each commit waits for your approval. In an interactive
session it is asked in the workspace (files and message, answer `y`) even with approval off;
where nobody can be asked (`duet run` with approval off, a session without a terminal) `git_commit` is
not offered. The read-only git tools always are. Details: [SECURITY.md](../SECURITY.md) (Git
tools).

### MCP servers

**MCP servers** (`[mcp.servers.<name>]` in the owner config, or declared by a
[native plugin](EXTENSIONS.md#optional-mcp-tool-servers)): Duet's
own Model Context Protocol client (stdio and streamable HTTP) starts each enabled server at run
start and offers its tools as `mcp__<server>__<tool>`. A stdio server runs in the command sandbox
(same hidden paths as commands, the workspace as working directory, no network unless `network =
true`, only the environment variables named in `env`); an HTTP server is reached from the host
(`https`, or `http` to loopback; credentials by variable name in `headers_env`).

```toml
[mcp.servers.everything]                  # a stdio server
command = "npx"
args = ["-y", "@modelcontextprotocol/server-everything"]
network = true                            # npx downloads the package
env = ["npm_config_cache"]                # e.g. npm_config_cache=.npm-cache (inside the workspace)

[mcp.servers.tickets]                     # a remote server
url = "https://mcp.example.com/mcp"
headers_env = ["Authorization=TICKETS_AUTH"]   # $TICKETS_AUTH holds "Bearer ..."
trust = "sensitive"                       # results stay on this machine
approve = "always"                        # ask before every call (oversight.approve = "risky")
```

`trust = "public"` (default): results are scanned and shown like public command output, and a call
whose arguments hold a placeholder or a known sensitive value is refused. `trust = "sensitive"`:
results stay on this machine (a handle and a local summary, like a `sensitive_data` command); for a
stdio server, placeholders in the arguments are resolved to their values (it is local). `approve`
(`writes` by default) decides which tools need the operator under `oversight.approve = "risky"`:
tools the server does not declare read-only, every tool (`always`) or none (`auto`); with `all`,
every call is asked. Tool descriptions and schemas are untrusted text: scanned and length-capped.
A server that fails to start, stops or hangs costs its calls (tool errors), never the run. Every
start and call is an audit event. `duet doctor` starts each stdio server (HTTP servers with
`--online`) and reports how many tools it offers; `duet config list` shows every server's settings.
Settings are owner-only and changes that start programs or reach servers need `--confirm`, e.g.
`duet config set mcp.servers.fs.command '"my-mcp-server"' --confirm`. Details:
[SECURITY.md](../SECURITY.md) (MCP servers).

### Language servers

**Language servers** (`lsp.*` settings; on by default when a server is installed): the frontier
gets `code_nav` (`definition`, `references`, `hover`, `symbols`, `workspace_symbols`,
`diagnostics`; answers are `path:line:col  text` lines, lines and character columns counted from 1
as `read_file` shows them) and `rename` (applied to every file at once, or to none). After
`edit_file`, `write_file` or `rename` on a file whose server is running, the result ends with a
short `diagnostics:` section (errors first). Duet looks for `rust-analyzer`,
`typescript-language-server`, `pyright-langserver` or `basedpyright-langserver`, `gopls` and
`clangd` on `PATH` (`duet doctor` lists what it found); a server starts on first use, runs in the
command sandbox without network, and is restarted once if it crashes (then the tools report it
unavailable and the run goes on). Other servers, or other commands for these languages:

```toml
# ~/.config/duet/config.toml (owner only; `duet config set lsp.servers.zig.command '"zls"' --confirm` works too)
[lsp.servers.zig]
command = "zls"
extensions = ["zig"]

[lsp.servers.rust]                         # replaces the built-in rust-analyzer lookup
command = "/opt/ra/bin/rust-analyzer"
env = ["RA_LOG"]                           # variables passed through by name
```

`duet config set --project lsp.enabled false` turns the tools off for one repository.

Servers read the workspace like checks do: protected source yes, sensitive files, `.git` and
`.duet` never. In hybrid mode an answer that points into a sensitive or sealed file shows only its
location, an interface-only file shows declarations only, and `rename` refuses to touch any of
them. Details: [SECURITY.md](../SECURITY.md) (Language servers).

### Sub-agents

**Sub-agents** (`subagents.*` settings; on by default): the frontier gets `delegate {task, mode,
paths?, budget?}`, which hands a sub-task to a sub-agent: a fresh loop that knows only the task
(none of the conversation), works with the same tools, boundary and sandbox, never more, and
answers with a report that becomes the tool result. Use it where a fresh context pays off:

- `mode: "read"` to investigate: "where is the retry policy applied, and which callers override
  it?", "why does `tests/export.rs` fail?", "summarize what `src/billing/` exposes". Read
  sub-agents read, search, use the git tools and `ask_local`, run commands with the repository
  read-only, and change nothing. Several asked for in one response run at the same time
  (`subagents.max_parallel`, 3 by default), so one turn can survey several parts of a large code
  base while the main conversation keeps only the reports.
- `mode: "write"` with `paths` (globs such as `src/export/**`) for one focused change: it edits
  and creates files matching `paths` only (anything else is refused), runs one at a time, and its
  result lists the files it changed (created, or `+added -removed` lines). Its commands are
  read-only too, so building and testing stay with the main loop.

Each sub-agent is held to `subagents.max_usd` and `subagents.max_minutes` (a `budget` in the call
can only lower them), and its spend and time count against the run's limits; one that runs out
stops, and the frontier is told. Sub-agents cannot delegate. `/undo` in a session reverts what a
writing sub-agent wrote in that turn, and a sub-agent cut off by a crash or Ctrl-C is rolled back
when the run or session continues. In the workspace their steps show under their id (`⇢ sub-agent
a1 (read): ...`, `[a1]  · read_file ...`, `⇠ sub-agent a1 completed`); the Runs screen shows them
indented the same way, and `summary.json` reports their requests and cost under `stats.subagents`.

```sh
duet config set --project subagents.max_parallel 2         # fewer at once
duet config set --project subagents.max_usd 0.5            # lower the per-sub-agent spend
duet config set subagents.model '"glm-5.3-flash"' --confirm # a cheaper model at the frontier endpoint
duet config set --project subagents.enabled false          # no delegate in this repository
```

In hybrid mode a sub-agent sees exactly what the main loop would (summaries, handles and
placeholders, never sensitive content); every request it makes passes the same outbound gate and
is in the same audit log, with `subagent_start` and `subagent_end` events (the task's hash, never
its text). Details: [SECURITY.md](../SECURITY.md) (Sub-agents).

### Local explorer

**Local explorer** (`explore.*` settings; off by default until measured): with a local model
enabled, the frontier gets `explore {question, paths?, depth?}`. The local model answers a
where/what/how question ("where is the retry policy applied?", "how does an order reach the
invoice?", "which tests cover the CSV export?") by searching and reading the repository itself, in
as many steps as it needs, and the frontier gets one result: a short answer and file:line
references with one-line notes, each checked against the files, with the code at the reference
quoted from open source files. A frontier that would otherwise list, search and read over several
turns (each resending the whole conversation) spends one.

```sh
duet config set explore.enabled true              # offer explore (hybrid and pass-through)
duet config set explore.quick_steps 12            # local model requests per quick call (default depth)
duet config set explore.thorough_steps 30         # ... per thorough call
duet config set explore.max_seconds 600           # time of a thorough call (a quick one: a third)
duet config set explore.max_read_kb 96            # tool output a thorough call reads (a quick one: half)
```

The explorer only reads: `read_file`, `list_files`, `search`, `code_nav` when language servers
are available, and the read-only git tools; no commands, no network, no delegation, no writes. It
reads sensitive files as they are (the local model may), but in hybrid mode its report is cleaned
as local-model output before the frontier sees it: a reference to `data/customers.csv line 3` is
fine (it points to the file's structure view), its content is not, values of the data are withheld
wherever the report names them, and protected source is never quoted (an interface-only file's
reference points to its skeleton, a sealed one's says only that it exists). Keep `explore.max_read_kb` within
your local model's context (data files take about one token per 2 bytes). A call that runs out of
steps or time before it reports tells the frontier which files it read. In pass-through mode the
report is shown as written: the explorer is only a cost tool there. Each call is an `explore`
audit event (the question's hash, steps, bytes, local seconds; never content), its local time is
in `summary.json` (`stats.ledger.explore`), and the Runs screen (`/runs`) shows each call's
outcome and counts. Details: [SECURITY.md](../SECURITY.md) (Local explorer).

### Images

**Images** (PNG, JPEG, GIF, WebP): `read_file` on an image in the workspace, `duet run --image
<path>` (repeatable), and `/image <path>` in a session (attached to your
next message). Every image is decoded, scaled to `images.max_side` (1568 px) and encoded again,
which drops its metadata (EXIF location, text chunks); files over 20 MB, or that stay over
3.75 MB encoded, are refused. Detectors cannot read text in an image, so in hybrid mode:

- by default the **local model describes** the image and the frontier gets the description,
  cleaned like any local output (values it recognizes become placeholders, names are withheld),
  plus a handle for `ask_local` follow-up questions. This needs a local model that reads images:
  `local.vision = true` (`duet config set local.vision true --confirm`).
- the frontier gets the **image itself** only when it is public: the operator attaches it with
  `--image-public <path>` or `/image --public <path>` (recorded in the audit log as the operator's
  decision), or `images.to_frontier = "public"` and it is a workspace file on a path that is
  neither sensitive nor protected. This needs `frontier.vision = true` (the `anthropic` and `openai`
  presets set it).
- an image on a **sensitive path** never goes to the frontier, marked public or not; one on a
  protected path is neither shown nor described.
- anything else is **refused with the reason** (for an attachment, before the run starts or
  before the message is sent), never sent anyway.

In pass-through mode an image goes to the frontier when `frontier.vision` is on and is refused
otherwise. Images the frontier sees are kept in the run directory by digest; transcripts and audit
records hold the digest, never the image. `duet doctor --online` tells you whether your models
really read images (see below). Details: [SECURITY.md](../SECURITY.md) (Images).

```sh
duet config set local.vision true --confirm            # a vision-language local model describes images
duet config set frontier.vision true --confirm         # the frontier model accepts images
duet config set --project images.to_frontier '"never"' # default; "public" also sends non-sensitive workspace images
```

### Local-model setup

**Getting a local model.** `duet setup` probes the local preset ports on `127.0.0.1`, reads
`/v1/models`, and offers the served text/chat models. Its defaults cover Ollama (11434), LM Studio
(1234), llama.cpp, LocalAI and MLX (8080), vLLM and oMLX (8000), Jan Desktop (1337), Jan CLI
(6767), GPT4All (4891), KoboldCpp (5001), and LiteLLM (4000). `DUET_LOCAL_PORTS` can replace
that list, or `duet setup --local-url URL` can select a custom endpoint. A remote local endpoint
must pass Duet's allowlist and transport checks before discovery contacts it. Jan and LiteLLM
can use `JAN_API_KEY` and `LITELLM_API_KEY`; only their variable names are saved. Model discovery
does not download, load or test a model.

With no `local.base_url` in your user config, `duet run` also looks for a server on this machine
only, lists what answered, and uses it for that run when exactly one model is on
offer (saying so, and recording it in the run). With several, or none, it prints the exact
`duet config set` commands and stops. It never writes configuration: `duet config preset <name>`
does that, through the same `--confirm` and audit path as any endpoint change.

### Privacy without a local model

**Privacy mode without a local model.** `duet config set local.enabled false` (a repository's own
config may set it too) runs hybrid mode with no local model: nothing is probed or contacted, no
model reads sensitive content, and the frontier sees it only as handles (their error lines with
values replaced, and line shapes or structure views) and synthetic samples. `ask_local` and `edit_protected` are refused, and the task tells
the frontier so. Top clearance, and `sensitivity.local_pii_pass` (which would otherwise be skipped
without a word), refuse to start. Turning it back on lets a local model read sensitive content
again, so it needs `--confirm`. The work is harder for the frontier without answers about the
data; the evaluation lane `duet-hybrid-nolocal` measures by how much.

### Structure views and masked output

**Structure of sensitive data** (`sensitivity.structure_views`, on by default; hybrid mode). The
frontier is told how sensitive data is laid out without seeing it, computed here without a model:
the task note outlines each sensitive file (`data/orders.json: JSON, an object; its records are the
84 elements of \`orders\`: orders[].placed_at (string, date-time yyyy-MM-dd'T'HH:mm:ss'Z'), …`),
and a file's view shows its structure: per field the type, how often it is present, null or empty,
a bucket of distinct values, lengths and shapes (`9999-99-99`, `Aa Aa`, `a.a99@a-999.a`; date
layouts as pictures), and anomalies (two date layouts in one field, integers above 2^53, embedded
delimiters and quotes, keys named `__proto__`). A log shows its line templates; a `.env` file how
each value is written (`integer, 3 digits`, `double-quoted; exported; secret-like, 32 chars`).
Counts and shapes, never values.

A data file's view also holds the first record of its **synthetic sample**, and
`synthetic_sample(handle="h3", rows=10)` gives more (`sensitivity.synthetic_rows`, 20 at most;
0 turns samples off): the file's records with every value replaced by a generated fake of the same
shape (valid dates in the same layout, card numbers and IBANs that pass their checks, emails at
`.test`, the same nulls, missing keys, quoting and delimiters), checked before it is shown to hold
no real value. It is not sensitive: use it as a test fixture. A fixture file made only of sample
lines stays readable to commands even under a sensitivity glob (a `.csv` under `tests/`) for as
long as it holds exactly those lines.

The output of a `sensitive_data` command is shown with every value masked (up to 80 lines):
values as shapes, secrets as `•••`, words kept when they are the public files', the task's, the
command's own or common toolchain words (`passed`, `expected`), numbers of up to two digits as written while `sensitivity.masked_numbers` (24 per
run) lasts, other numbers as `9`s. A short output (at most 200 characters: a count, a match, a yes
or no) is a probe of the data: the run shows at most `sensitivity.output_probes` (12) of them, then
withholds each (the view no longer depends on it; each probe is an `output_probe` audit event). A
project may lower all of these; raising them, or turning views on again, needs `--confirm`.
`cargo run -p duet-boundary --example structure_report -- <workspace> <task file> <state dir>`
prints what the frontier would be shown of a workspace's sensitive files.

### Condensed command output

**Condensed command output.** In hybrid mode, test, build and install output the frontier may see
is shown condensed (`context.condense_output`, on by default): failing tests with their assertion
messages, the first compiler or type error of each kind whole and later ones by location and label,
each warning once per place, summary counts, the exit code and the sandbox's notes. Passing tests,
compile, download and progress lines, repeated warnings, the middle of a long failure and deep
stack frames are left out; each omitted run is one marker naming its lines
(`[lines 4-12 omitted: 8 passing tests, 1 routine line]`), and the view ends with
`condensed from N lines; read_raw(handle="h3", start_line=..., end_line=...) for the full output`.
Formats: `cargo test` and rustc (`cargo build`, `check`, `clippy`), pytest, unittest, jest, vitest,
mocha, `node --test`, `go test`, tsc, eslint, npm, pnpm and yarn, Maven, Gradle, and any output that
reports pass/fail counts. Shown as before: output under about 300 tokens; output in no recognized
format (over `sensitivity.bulky_tokens`: the first lines and an outline, as before); output a view
would cut by less than 30%; output of a command that shows or searches files (`cat`, `grep`,
`git diff`) or that the model piped through a filter (`| grep FAIL`; `| head` and `| tail` are
fine); and output held as sensitive (a `sensitive_data` command's, or over 6,000 characters while
command output is sensitive). Pass-through mode shows command output as it is. On the recorded Gate 2
and X1/X2 runs the condensed outputs shrank by 48%, but only 43 of 1,635 command outputs qualified
(the models already cut test output with `| tail`, and most command output is file reading):
0.26% of the hybrid lane's input tokens.

### Diagnostics

**`duet doctor`** checks the configuration and its origins, the config audit chain, settings looser
than their defaults, the frontier endpoint and whether its key variable is set (the value is never
printed), local-endpoint trust (loopback, allowlist, the plain-HTTP rule), the sandbox, git (and,
outside a repository, what works without one), supported instruction files and overrides, disk
space, the audit chains and anchors of the latest runs, run data past retention, the approval
mode, and whether release signing keys are present (a warning in a build made by
`tools/release.sh`; a note in a development build, since no release has been published). It uses no
network by default. `--online` adds one model listing per configured server (and the local model's
context window), and a cache-reuse check that sends the frontier and the local model the same short
built-in prompt twice (nothing from the workspace) and reports the cached input tokens of the
repeat, warning when there are none, and a vision check that shows the local model (and the
frontier, when `frontier.vision` is on) two generated one-colour images and asks for the colour: a
model whose answers do not match fails while its `*.vision` setting says it reads images (some
servers accept image parts and silently drop them; the model then describes an image it never
saw). Exit code: 0 pass, 1 warn, 2 fail.

Live smoke tests, one per local backend, run with
`DUET_LIVE_OLLAMA_URL=http://127.0.0.1:11434/v1 cargo test -p duet-boundary --test backend_smoke -- --ignored`
(also `LMSTUDIO`, `LLAMACPP`, `VLLM`, `OMLX`, `MLX`; `DUET_LIVE_<BACKEND>_MODEL` picks the model).

## Models

`duet setup` looks for provider keys by environment variable name. One key lets it select a cloud
provider automatically; several prompt for the intended recipient. `--yes` does not guess among
several cloud accounts. It fetches only that provider's model listing, chooses the existing or
preset model if served, otherwise offers a short ranked list of candidate chat models. A listing
that explicitly says tools or chat are unsupported excludes that model. This is a suggestion,
not a tool-use benchmark. Authentication rejection stops setup. If a listing is unavailable for
another reason, a preset model or `--model ID` can be used. Image input is enabled automatically
only for the preset's known default or when the listing advertises image input. Run
`duet doctor --online` to check the selected model's listing, context, cache and image behavior.

| Frontier preset | Key variable | API dialect |
|---|---|---|
| `zai` | `ZAI_API_KEY` | OpenAI-style chat |
| `anthropic` | `ANTHROPIC_API_KEY` | Anthropic Messages |
| `openai` | `OPENAI_API_KEY` | OpenAI Responses |
| `gemini` | `GEMINI_API_KEY` | OpenAI-style chat |
| `openrouter` | `OPENROUTER_API_KEY` | OpenAI-style chat |
| `deepseek` | `DEEPSEEK_API_KEY` | OpenAI-style chat |
| `xai` | `XAI_API_KEY` | OpenAI-style chat |
| `mistral` | `MISTRAL_API_KEY` | OpenAI-style chat |
| `groq` | `GROQ_API_KEY` | OpenAI-style chat |
| `cerebras` | `CEREBRAS_API_KEY` | OpenAI-style chat |
| `together` | `TOGETHER_API_KEY` | OpenAI-style chat |
| `fireworks` | `FIREWORKS_API_KEY` | OpenAI-style chat |
| `qwen` (Singapore endpoint) | `DASHSCOPE_API_KEY` | OpenAI-style chat |

`frontier.dialect` also supports compatible custom endpoints. The preset URLs and protocol choices
follow the providers' published APIs; live provider/model combinations still need checking with
your account. Mistral, Together and Fireworks deliberately have no hardcoded model: setup uses a
live listing or `--model`. For Fireworks, setup also tries its public account-scoped model
catalog when the inference endpoint has no listing; an account-specific deployment may still need
`--model`. These presets are tested against scripted protocol streams; they are not
a claim that every hosted model supports Duet's tool calls. The outbound gate filters, checks and
audits the exact request body in each dialect.

The default local model is `omlx-coding` (Qwen 3.8 27B on oMLX), chosen with `duet local-eval`.
Only oMLX has served live runs so far; other local backends have preset/discovery tests and need
a live smoke test on the operator's machine. `omlx-coding` does not read images, so
`local.vision` stays off. Generic OpenAI-compatible local servers work on loopback or an
allowlisted host; plain HTTP away from loopback needs `local.allow_plaintext`.

## Costs and pricing

Frontier token estimates use OpenRouter's public catalog for the selected model, including cache
reads/writes and conditional context/time rates. Exact model slugs are preferred; unqualified
names resolve only when unique. The catalog is refreshed when its saved copy is at least 24 hours
old. If unavailable, Duet uses the saved copy or the bundled 2026-09-30 snapshot, labeling its source
and age in **Session** and `/status`. This public request sends no prompts or API credentials.
Top-clearance runs and loopback frontier endpoints never refresh the catalog. To disable the
lookup elsewhere, set `pricing.offline true`.

If your endpoint uses a model alias, map its pricing to the exact OpenRouter slug:

```sh
duet config set pricing.frontier_model '"z-ai/glm-5.3-flash"'
```

This changes the price lookup, not the model sent to the endpoint. Clear it with `duet config set
pricing.frontier_model '""'` when returning to automatic resolution. Unknown or ambiguous frontier
prices stop the run before a model request instead of silently charging zero. A delegated frontier
model uses its own catalog price; optional frontier security opinions use the main frontier quote.

For a direct provider whose rate differs from OpenRouter's estimate, set the rate published by that
provider. The override applies only to the exact model and endpoint; both input and output rates
must be positive:

```sh
duet config set pricing.manual_model '"my-model-id"'
duet config set pricing.manual_base_url '"https://api.example.com/v1"'
duet config set pricing.manual_input_usd_per_million 2.00
duet config set pricing.manual_output_usd_per_million 8.00
# optional: pricing.manual_cache_read_usd_per_million and pricing.manual_cache_write_usd_per_million
```

Rates are USD per million tokens. If cache rates are left at zero, Duet uses the input rate for
those tokens. A quote is a budget estimate, not the provider invoice. `duet doctor` shows its
source and warns when no enforceable price is available.

Local input/output rates default to **$0**. Set your own USD-per-million-token estimates in F2
settings, or for example:

```sh
duet config set local.input_usd_per_million 0.20
duet config set local.output_usd_per_million 0.80
```

Rates apply after restarting/resuming the session. All local roles share the meter, including
readers, explorers, reviews, compaction and top-clearance coding. Cached input uses the same local
input rate. Session and `/status` show local tokens/rates/cost, frontier cost and the combined
estimate. Local costs stay outside the frontier dollar budgets. `economics.json` in the private
run directory stores the cumulative local estimate; `pricing-history.jsonl` records price bases
on each start/resume. Changing rates does not reprice previously recorded spending. Sessions from
older releases start local accounting with new work; historical costs are not reconstructed.

These are token-cost estimates, not invoices. Provider counts are used when available; missing
counts and retries that produced partial output use estimates. When a local request is canceled,
usage from completed attempts is retained and `local_usage_unknown_requests` in the run summary
counts requests whose in-flight token charge remains unknown. Session and `/status` also flag that count;
the displayed dollar figure may be low by the unreported charge. UTC tiers use the accounting
time. Request fees, non-token services, subscriptions,
taxes and endpoint-specific discounts are outside the estimate; a model with a known fixed request
fee is rejected. The frontier budget continues to use the recorded per-response cost.

## Configuration

Every setting is defined in one registry and editable with `duet config` or in the workspace (`/settings`):
models, sensitivity rules and detectors, protected paths and IP levels, budgets, retention.
Credentials and endpoints live only in your user config (`~/.config/duet/config.toml`); a
repository's `.duet/config.toml` can make privacy stricter but never looser.

### Long conversations

When a conversation grows past `context.mask_at` (0.7) of `context.window_tokens` (200K), old
tool results are replaced by short stubs that name the call, oldest turns first. With
`context.compaction` on (off by default until it is measured), a long conversation is also
condensed by the local model:

```sh
duet config set context.compaction true
duet config set context.compact_at 100000   # estimated request tokens that trigger it
duet config set context.compact_to 0.4      # what it is brought down to, as a fraction of that
```

Past `context.compact_at`, masking is tried first; if it cannot bring the conversation down to
`context.compact_to` of the threshold, everything between the first message and the recent turns
is replaced by a working summary the local model writes (the task, what was done and why, what
failed, the state, what is open, and the exact paths and names). The recent turns stay verbatim,
and the frontier reads files again when it needs them. It needs a local model (hybrid mode with `local.enabled`); a
failure leaves the conversation as it was and masking goes on. Keep `context.compact_at` below
`context.mask_at` of the window. Each compaction appears in the run's progress, on the Runs screen, as
a `compaction` audit event and in `summary.json` (`stats.compactions`).

To see what compaction would do to a recorded run, replay its transcript offline with your local
model:

```sh
cargo run -p duet-cli --example compaction_replay -- .duet/runs/<run-id> --out /tmp/compaction
```

It reports each event (tokens before and after, local seconds), request tokens over the run with
and without compaction, and writes each summary to the output directory.
