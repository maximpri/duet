# Duet

**Frontier-level coding results, with sensitive information processed only by a local model.**

> Status: **in development, not released.** The `duet` CLI, the security engine, IP levels and the
> evaluation harness work. The privacy gate and the quality gate (Gate 2) passed on earlier builds;
> quality has not been re-measured on the current build. Duet costs more than the frontier alone
> (a measured privacy premium, below). Local backend presets, a no-config bootstrap, `duet doctor`
> and the `duet tui` terminal UI exist. Progress and measurements: [docs/PLAN.md](docs/PLAN.md) §10;
> what is verified and what is not: [docs/ACCEPTANCE.md](docs/ACCEPTANCE.md).

## What it is

Duet is a coding agent for repositories that contain things you shouldn't send to a cloud model —
credentials, customer data, production logs, regulated data, or your most valuable source code.

- A **frontier model** plans, decides every step and writes the code, so you get frontier-quality
  work.
- A **local model** on your machine is the only model that ever reads sensitive content. It
  summarizes it, answers questions about it and rewrites protected code on request; it has no tools
  and never decides what to do next.
- A **single outbound gate** checks every byte before it leaves your machine and records it in a
  tamper-evident audit log you can inspect.

Today you usually choose between an agent that sends everything to the cloud and a local-only agent
with lower quality. Duet is built so you don't have to.

## How it works

```
frontier model ◄── outbound gate ◄── what the frontier is allowed to see
                     (scan, redact,        │
                      audit log)           │  raw code, placeholders, summaries, answers
                                           │
your repository ──► security engine ──────┘
                        │  secrets, personal data, logs, data files, protected code
                        ▼
                   local model (on your machine): reads, summarizes, answers
```

- **Secrets** appear to the frontier as placeholders like `⟨secret:DB_URL#1⟩`. When it writes code
  or config using a placeholder, Duet fills in the real value locally, and only in places where a
  secret belongs.
- **Logs and data files** appear as a handle with error lines and values replaced, their repeated
  line shapes and a local summary. The frontier asks the local model questions about them
  (`ask_local`), but never sees the raw content. Optionally (`sensitivity.local_brief`, off by
  default) the local model also briefs the frontier at the start of a run on what the sensitive
  files show for the task, with values withheld.
- **Commands cannot read sensitive files** (OS sandbox). A command that must, such as the program
  run on the real data, is marked `sensitive_data`: its output stays local behind a handle, the
  files it writes become sensitive, and placeholders in it are filled in locally. No command can
  read Duet's own state (`.duet/`).
- **Values you type** (a card number in the task, a name in a chat message) appear to the frontier
  as placeholders too; it works with them by asking the local model, which reads your message with
  the real value. What Duet prints for you (the run's summary, chat replies) shows your values.
- **Large public results** (long files, outputs, listings, searches) appear as the first lines and
  an outline; the frontier reads the ranges it needs.
- **Protected source code** can be shown as interfaces only (signatures, types, docs) or hidden
  entirely. Edits to protected code are implemented locally against tests the frontier writes.
- **Everything that leaves** is logged in a hash-chained audit log, together with the security
  decisions of the run, and its head is anchored outside the repository: `duet audit show <run>`,
  `duet audit verify <run>`.
- **Secure by default:** loosening a privacy setting needs `--confirm` and is recorded; the
  boundary-off passthrough mode needs `--no-privacy`; a LAN model over plain HTTP is refused unless
  the owner opts in.

Duet withholds sensitive content; it does not obfuscate it. What the frontier can still infer —
that a file exists, its shape, the intent of your task — is documented in `SECURITY.md`.

## Goals Duet is measured against

| Goal | Measure |
|---|---|
| Frontier-level results | On a suite of tasks from 15-minute fixes to multi-hour changes, code quality (blind judge) within 2/30 of the same frontier model alone, and hidden-test pass rate not behind it on most tasks |
| Sensitive data stays local | Zero planted canaries in outbound traffic, measured by an independent logging proxy; every request is also in Duet's audit log |
| Known cost | The extra cost of privacy is measured and reported: the cost ratio against the same frontier model alone (API list prices incl. caching, plus local electricity). Duet is not expected to be cheaper |
| Reliable | Every run ends as completed, failed with a reason, or budget-stopped; interrupted runs resume |

So far (frontier `glm-5.3-flash`): quality matched the frontier alone in Gate 2 (18 paired runs,
one judge, build `a5346f4`); later builds have not been judged. No planted canary left in any of
74 valid hybrid runs across seven batches, while the frontier alone sent canaries in every run on
the same tasks. Duet is not cheaper than the frontier alone: privacy costs extra frontier turns,
because the frontier must ask about data it cannot read, and offloading bulky content to the local
model saves less than those turns cost. In the measured batches Duet cost about 1.5× to 2.4× as
much as the frontier alone, depending on the tasks (ratio of mean cost per run; no interval has
been published yet). Full results will be published in `docs/BENCHMARK.md` (milestone M5), with raw
data and the method needed to reproduce them.

## Usage

```sh
duet chat                       # code in a conversation (a session; hybrid mode by default)
duet chat --resume              # continue the most recent open session (or --resume <id>)
duet run "fix the failing billing export"      # one-shot: work to a terminal state, no conversation
duet audit show <run>           # see exactly what was sent to the frontier, and the security events
duet audit verify <run>         # check the hash chain and its anchor
duet audit disclosure <run>     # what was withheld from the frontier, by class (counts only)
duet resume <run>               # continue an interrupted one-shot run
duet config list                # every setting, its value and where it came from
duet config set --project ip.interface_only '["src/pricing/**"]'
duet config preset              # local backends and frontier providers (zai, anthropic, openai)
duet config preset ollama --model qwen3:8b --confirm   # point the local role at one (audited)
duet config preset anthropic --confirm                 # frontier endpoint, model, key variable, dialect
duet doctor                     # pass/warn/fail with a fix per check; no network (--online, --json)
duet tui                        # settings, IP levels, audit viewer, run view; sessions and runs
duet local-eval                 # measure the configured local model in its reading roles
duet purge                      # delete raw run data older than the retention period
```

`duet run --mode passthrough --no-privacy` runs the frontier alone with the boundary off (the
evaluation baseline).

### Coding with duet: sessions

A session is one conversation in one workspace. You give a task, duet works on it with its tools
(each step shows as a progress line), and the turn ends with duet's reply, a question for you, or a
finished task; your next message continues with everything said and done so far.

```text
$ duet chat
duet chat: a new session starts with your first message. /help lists commands.
you> The CSV export drops the last row. Find out why and fix it.
  · search fn export_csv
  · read_file src/export.rs
  ◦ withheld from the frontier: read_file result sensitive, held locally (a summary was sent)
duet asks: Should an empty trailing line count as a row?
      options: yes / no
      (your next message is the answer)
you> no
  · edit_file src/export.rs
  · run_command cargo test export
  · finish (running the checks)
duet finished: The loop stopped one row early; it now reads to the end, with a test.
you> /diff
...
you> Also add a header row option.
```

- **Talking to duet.** Every message is a turn. duet ends it with `duet:` (a reply), `duet asks:`
  (a clarifying question: your next message is the answer) or `duet finished:` (it called `finish`
  and `checks.commands` passed). End a line with `\` to continue the message on the next line;
  `//text` sends a message that starts with `/`.
- **Steering while it works.** Type while duet is working: the message is delivered after the
  current step (its tool results are recorded first; a running command is never cut short), and
  duet takes it into account from its next step. Several messages typed meanwhile arrive together,
  in order. `/stop` ends the turn after the current step; **Ctrl-C** ends it at once and kills a
  running command. Either way the session stays open and duet is told what happened.
- **Commands** (never sent to the model): `/status` (turns, tokens, cost and working time against
  the budgets), `/diff` (the workspace against the last commit; sensitive files are named, not
  shown), `/undo` (reverts the files the last turn wrote through its tools; repeat to go back
  further; changes made by commands are not reverted), `/quit` (leave; the session stays open),
  `/close` (end it for good), `/help`.
- **Resuming.** `/quit`, the end of input or Ctrl-C twice at the prompt leaves the session open;
  `duet chat --resume` continues the latest open one (with a recap of its last turns), `--resume
  <id>` a given one, also after a crash.
- **Privacy.** You see real values in your terminal; the frontier gets your messages the way it
  gets task text: detected secrets and personal data become placeholders, and values it has seen
  as placeholders stay placeholders. duet's replies show the real values back to you. See
  [SECURITY.md](SECURITY.md) (Operator messages).
- **Limits.** Each turn is held to `limits.frontier_usd` and `limits.wall_clock_minutes`; the
  session to `session.frontier_usd` and `session.wall_clock_minutes` (time duet works; time
  waiting for you does not count). A turn stopped by a limit leaves the session open; a spent
  session budget ends it until the budget is raised. `oversight.approve` asks in the conversation
  (answer `y` on the next line); with approval on, `duet chat` needs a terminal.

`duet run` stays the one-shot path (scripts, evaluation): no conversation, and its requests are
unchanged.

**`duet tui`** has seven screens: Models (frontier and local settings, with `duet doctor`
offline; `o` adds the online checks of connection and context window; `l` looks for local
servers on loopback (the preset ports, as bootstrap does) and `[` `]` `u` point the local role
at one; `c` runs the cache-reuse probe, two identical short requests to the local model, only
when pressed), Sensitivity (globs, detectors, custom detector patterns, raw-output commands,
secret sinks, bulky thresholds; `t` tests a path — sensitive or not and which pattern matched —
and `s` tests sample text, showing what the detectors and your patterns would replace), IP
levels (the workspace tree as git sees it, so `.gitignore` applies; `i` / `s` mark a file or
directory interface-only / sealed, with a preview of the skeleton the frontier would see),
Limits, Data (retention; `x` purges the raw data of runs older than `data.retention_days`, `X`
of every run, after listing them and asking; audit logs are kept), Audit (each run's records and
outbound request summaries as stored, and `v` to verify the hash chain and its anchor) and Run.
The Run screen has two panels: the run as it is written (turns, tool calls, results, and a feed
of withheld content) and the files it changed with added/removed line counts above the selected
file's diff, with line numbers, following the file the run wrote last. Files that are sensitive,
or were produced by a command that read sensitive data, are listed as held locally and their
content is not shown. `←` `→` switch panels, arrows pick a file, `PgUp` `PgDn` (or `J` `K`)
scroll the diff; a finished run reads the same way. `n` starts a session: type the first message,
pick the mode (hybrid by default; passthrough asks you to acknowledge that the privacy boundary is
off), and Duet launches `duet chat` in the background (output in `.duet/tmp`) and opens it here.
The main panel then shows the conversation (your messages, duet's replies and questions, steering
with the step it arrived after) above an input box: `i` types a message (sent with `Enter`; while
duet works it steers the turn), `s` stops the turn after its current step, `x` (or `Ctrl-C` in the
box) stops it at once, `/status` reports turns and cost, `Esc` leaves the box. `r` resumes the
selected session. If you quit, the session finishes its current turn and stays open. `o` starts
a one-shot `duet run` instead, which keeps running if you quit. With `oversight.approve` on, use
`duet chat` or `duet run`, which have the terminal for approvals. The settings screens are generated from the registry and
show where each value comes from (default, owner or project). Edits take the same path as
`duet config set`: a change that loosens privacy shows its diff and needs `y`, `p` switches edits
to the project file (which only tightens and never takes owner-only keys), and every applied change
is recorded in the owner's config audit log. It needs an interactive terminal of at least 80x24;
without one it says so and exits with status 1.

**What the detectors find.** Duet's own secret and personal-data detectors, the gitleaks rule set
(221 rules for specific services' credentials, used as data: pinned in
`crates/duet-boundary/rules/`, updated with `tools/update-rules.sh <version>`), international
phone numbers, IBANs, the main EU/UK national IDs with their check digits, IPv6 and labelled postal
addresses. `duet doctor` shows the rule set in use; [SECURITY.md](SECURITY.md) (Detection) lists
everything with measured recall, false positives and limits. A person's name in free text has no
shape to detect: with `sensitivity.local_pii_pass = true` (off by default; it costs local model
time on every public result with prose) the local model marks names and postal addresses in the
prose of public content, and they become placeholders like detected values.

**Custom detector patterns.** `sensitivity.custom_patterns` takes regular expressions for values
only you know are sensitive (customer ids, internal host names): every match becomes a `data`
placeholder, in sensitive and public text alike. A project may add patterns; removing one loosens
privacy and needs confirmation.

**Operator approval** (`oversight.approve`, owner config only; default `off`): with `risky`, Duet
asks y/N on the terminal before a `sensitive_data` command, a protected edit, or a write to anything
other than an ordinary source or test file; with `all`, before every command and write. A refusal
is returned to the model as a tool error, and every decision is in the run's audit log. A run with
approval on and no terminal refuses to start. Details: [SECURITY.md](SECURITY.md) (Oversight).

**Web tools** (`web.*` settings; on by default): the frontier gets `web_fetch` (a public page
as text; HTML is converted with links kept; `start_line`/`end_line` read part of a long page) and,
when a search backend is configured, `web_search` (title, URL and snippet per result). Requests are
made by the host, never by commands (commands still have no network): `GET` only, `http`/`https`,
public addresses only (checked after DNS and on every redirect), at most `web.max_bytes` per
response and `web.timeout_secs` per request. Search backends:

```sh
duet config set web.search.backend '"searxng"' --confirm                 # your own SearXNG instance
duet config set web.search.searxng_url '"http://127.0.0.1:8888"' --confirm  # JSON format enabled
duet config set web.search.backend '"brave"' --confirm                   # Brave Search API; key in $BRAVE_API_KEY
duet config set web.allowlist_private '["wiki.corp", "10.20.0.0/16"]' --confirm   # intranet hosts for web_fetch
duet config set --project web.enabled false                              # no web tools in this repository
```

In hybrid mode a URL or query holding a placeholder or a known sensitive value is refused before
anything is sent, and fetched content is scanned like public content and shown as untrusted data.
Every call is an audit event (host, bytes, outcome). Details: [SECURITY.md](SECURITY.md) (Web tools).

**Git tools** (when the workspace is a git repository): `git_status`, `git_log {path?, rev?,
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
duet config set oversight.approve '"risky"'                     # git.commit = "ask" (default): offered, each commit asks
duet config set git.commit '"allow"' --confirm                  # commit without asking (e.g. with approval off)
duet config set --project git.commit '"off"'                    # no git_commit in this repository
```

With the default `git.commit = "ask"` and approval off (`oversight.approve = "off"`), nobody can be
asked, so `git_commit` is not offered; the read-only git tools always are. Details:
[SECURITY.md](SECURITY.md) (Git tools).

**MCP servers** (`[mcp.servers.<name>]` in the owner config; the only plugin mechanism): Duet's
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
[SECURITY.md](SECURITY.md) (MCP servers).

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
them. Details: [SECURITY.md](SECURITY.md) (Language servers).

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
when the run or session continues. In `duet chat` their steps show under their id (`⇢ sub-agent
a1 (read): ...`, `[a1]  · read_file ...`, `⇠ sub-agent a1 completed`); the TUI Run view shows them
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
its text). Details: [SECURITY.md](SECURITY.md) (Sub-agents).

**Getting a local model.** With no `local.base_url` in your user config, `duet run` looks for a
server on this machine only (127.0.0.1 on the preset ports 11434, 1234, 8080 and 8000, or
`DUET_LOCAL_PORTS`), lists what answered, and uses it for that run when exactly one model is on
offer (saying so, and recording it in the run). With several, or none, it prints the exact
`duet config set` commands and stops. It never writes configuration: `duet config preset <name>`
does that, through the same `--confirm` and audit path as any endpoint change.

**`duet doctor`** checks the configuration and its origins, the config audit chain, settings looser
than their defaults, the frontier endpoint and whether its key variable is set (the value is never
printed), local-endpoint trust (loopback, allowlist, the plain-HTTP rule), the sandbox, git, disk
space, the audit chains and anchors of the latest runs, run data past retention, the approval
mode, and whether release signing keys are present (a warning in a build made by
`tools/release.sh`; a note in a development build, since no release has been published). It uses no
network by default. `--online` adds one model listing per configured server (and the local model's
context window), and a cache-reuse check that sends the frontier and the local model the same short
built-in prompt twice (nothing from the workspace) and reports the cached input tokens of the
repeat, warning when there are none. Exit code: 0 pass, 1 warn, 2 fail.

Live smoke tests, one per local backend, run with
`DUET_LIVE_OLLAMA_URL=http://127.0.0.1:11434/v1 cargo test -p duet-boundary --test backend_smoke -- --ignored`
(also `LMSTUDIO`, `LLAMACPP`, `VLLM`, `OMLX`, `MLX`; `DUET_LIVE_<BACKEND>_MODEL` picks the model).

### Models

- **Frontier:** z.ai `glm-5.3-flash` by default. `frontier.dialect` selects the API the endpoint
  speaks: `chat` (any OpenAI-compatible Chat Completions endpoint, the default), `anthropic`
  (Anthropic Messages, with prompt-cache breakpoints on the stable prefix and a rolling one on the
  conversation) or `responses` (OpenAI Responses, stateless). Presets: `zai` (`glm-5.3-flash`),
  `anthropic` (`claude-opus-5-5`), `openai` (`gpt-5.5`). The outbound gate filters, checks and
  audits the exact body of whichever dialect is used. The Anthropic and Responses dialects are
  tested against scripted streams only; no live run has used them yet.
- **Local:** any OpenAI-compatible Chat Completions server (oMLX, LM Studio, llama.cpp, vLLM,
  Ollama) on loopback, or on a host the owner allowlists; plain HTTP to a non-loopback host needs
  `local.allow_plaintext`. The default, `omlx-coding` (Qwen 3.8 27B on oMLX), was chosen with
  `duet local-eval`. Only oMLX has served live runs so far; the other servers are supported through
  presets and discovery tested against mocked replies, and their live smoke tests have not been run
  yet.

### Configuration

Every setting is defined in one registry and editable with `duet config` or `duet tui`:
models, sensitivity rules and detectors, protected paths and IP levels, budgets, retention.
Credentials and endpoints live only in your user config (`~/.config/duet/config.toml`); a
repository's `.duet/config.toml` can make privacy stricter but never looser.

## Documentation

| Document | Contents |
|---|---|
| [docs/TARGET_STATE.md](docs/TARGET_STATE.md) | What Duet is when finished: north star, security engine, IP levels, configuration |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Crates, types, turn lifecycle, boundary internals, state on disk, invariants |
| [docs/PLAN.md](docs/PLAN.md) | Milestones, gates, evaluation method, risks, verification |
| [docs/DOGFOOD_SUITE.md](docs/DOGFOOD_SUITE.md) | Test tasks by complexity tier, canaries, metrics, statistics |
| [SECURITY.md](SECURITY.md) | Threat model, secure defaults, known limits, advisories, reporting a vulnerability |

## Building

Rust only.

```sh
cargo build --release
tools/gate.sh        # formatting, lints, tests, license and provenance checks
```

A release is signed and comes with an SBOM. The operator provides an SSH signing key (never stored in
the repository):

```sh
tools/release.sh 0.1.0 --key ~/.ssh/duet_release   # gate, --locked release build, SBOM, SHA256SUMS + signature
tools/verify-release.sh dist/duet-0.1.0            # signature (allowed_signers) and every checksum
tools/sbom.sh --out duet.cdx.json                  # CycloneDX SBOM alone (offline, no plugin)
```

To verify a release, put the published signer line in `~/.config/duet/allowed_signers`. Details:
[SECURITY.md](SECURITY.md) (Supply chain).

## Development

There is no hosted CI: `tools/gate.sh` is the gate, and every commit must pass it. Nothing else
enforces that, so the first step after cloning is to install the hooks that run it:

```sh
tools/install-hooks.sh --pre-commit   # pre-push: full gate; pre-commit: tools/gate.sh --fast
```

Then, day to day:

```sh
tools/gate.sh --fast                  # format, license, privacy and provenance checks (seconds)
PROPTEST_CASES=5000 cargo test -p duet-boundary --test no_canary   # a deeper property run
tools/fuzz.sh 60                      # each fuzz target for 60 s (see tools/fuzz.sh --help)
cargo test -p duet-boundary --test corpus -- --nocapture   # detection recall and false positives
tools/update-rules.sh v8.30.1         # check a gitleaks release against the vendored rules (--apply to take it)
```

The hooks are not installed automatically (a clone never runs code on its own). They live in the
repository's hooks directory, so every worktree of a clone shares them;
`tools/install-hooks.sh --uninstall` removes them and `--no-verify` skips them once. Property tests
run in the gate with small case counts; `PROPTEST_CASES` raises them. Fuzzing is not part of the gate (it needs time, and cargo-fuzz needs a
nightly toolchain; without one `tools/fuzz.sh` builds the targets on stable).

## License

GPL-3.0-or-later.
