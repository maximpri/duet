# Using Duet

How to run Duet, code with it in sessions, and configure its tools, models and settings. Why Duet
exists and how it keeps sensitive information local: [README](../README.md). The threat model and
every security rule in detail: [SECURITY.md](../SECURITY.md).

## Commands

```sh
duet                            # the workspace: code in a conversation (hybrid mode by default)
duet "fix the failing export"   # the same, with the first message given
duet --resume                   # continue the most recent open session (or --resume <id>)
duet run "fix the failing billing export"      # one-shot: work to a terminal state, no conversation
duet run --quiet "..."          # the same without progress on standard error (the summary is unchanged)
duet run --image shot.png "fix this layout bug"  # attach an image (repeatable; --image-public: see Images)
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
duet local-eval                 # measure the configured local model in its reading roles
duet purge                      # delete raw run data older than the retention period
```

`duet run --mode passthrough --no-privacy` runs the frontier alone with the boundary off (the
evaluation baseline). A repository can forbid it, for new and resumed runs and sessions alike:
`duet config set --project frontier.allow_passthrough false` (turning it back on is the owner's,
with `--confirm`).

**Project instructions.** Put how to work in a repository (conventions, the commands that build and
test it, its layout) in `DUET.md` at its root, and your own standing instructions for every
repository in `~/.config/duet/DUET.md` (next to your config; `$DUET_CONFIG_HOME/DUET.md` when that
is set). Duet gives both to the frontier at the start of every run, session and sub-agent, ahead of
the task and framed as instructions (yours first); they never change during the conversation, so
the request prefix stays cached. Each is limited to 16 KiB (a longer one is cut at a line, with a
note); `duet doctor` says which it found. The repository's file is repository text: in hybrid mode
it is scanned like any file (a key in it reaches the frontier as a placeholder), and it cannot
change a setting, the policy or the sandbox, whatever it says. Details:
[SECURITY.md](../SECURITY.md) (Project instructions).

**Without git.** Duet works in any folder. Outside a git repository, files are listed and searched
by a walk that honours `.gitignore` and `.ignore` files and skips `.git`, `.duet` and dependency and
build-output directories (`node_modules`, `target`, `dist`, `__pycache__`, `.venv`, ...); `diff`
and `/diff` compare the files duet wrote with their content before the run (changes made only by
commands are not shown); `/undo`, resume and the audit work as in a repository; the git tools are
not offered (`git init` adds them).

## Coding with duet: the workspace

`duet` opens the workspace: one session (one conversation in one workspace) in full screen. You
give a task, duet works on it with its tools (each step shows as a progress line), and the turn ends
with duet's reply, a question for you, or a finished task; your next message continues with
everything said and done so far.

```text
 duet · hybrid · glm-5.3-flash ▸ omlx-coding                    turn 2 · $0.0184 of $20.00
╭ conversation ─────────────────────────────────────╮  Changes   Privacy   Session  Ctrl-T
│ you> The CSV export drops the last row. Fix it.   │┌ changed files 1 (+3 -1) ──────────┐
│   · search fn export_csv                          ││edit src/export.rs  +3  -1         │
│   · read_file src/export.rs                       │└───────────────────────────────────┘
│   ◦ withheld from the frontier: read_file result  │┌ src/export.rs (edited) ───────────┐
│     sensitive, held locally (a summary was sent)  ││ 41 - for i in 0..rows.len() - 1 { │
│ duet asks: Should an empty trailing line count?   ││ 41 + for i in 0..rows.len() {     │
│ you> no                                           ││    …                              │
│   · edit_file src/export.rs                       ││                                   │
│   · finish (running the checks)                   ││                                   │
│ duet finished: The loop stopped one row early.    ││                                   │
╰───────────────────────────────────────────────────╯└───────────────────────────────────┘
╭ message ───────────────────────────────────────────────────────────────────────────────╮
│ › Also add a header row option.                                                         │
╰────────────────────────────────────────────────────────────────────────────────────────╯
 ⏎ send · Alt-⏎ new line · Tab complete · PgUp/PgDn scroll · Ctrl-T panel · /help
```

- **The screen.** The status bar: the mode, the frontier and local models, the turn, and the cost
  against the session budget. The conversation: your messages, duet's steps and what the boundary
  withheld from the frontier, duet's text as the frontier writes it (formatted lightly: headings,
  lists, quotes, code blocks set off with a bar, inline code and bold), how each turn ended; while
  duet works, a status line shows the turn's time, its cost so far and what runs now. It follows new
  output; PgUp/PgDn or the mouse wheel scroll back (it then stays put and says how many rows are
  below). The side panel (Ctrl-T cycles it; it opens by itself on a window 110 columns or wider):
  **Changes** lists the files the session changed with added/removed line counts above the selected
  file's diff (Ctrl-↑ ↓ pick a file, Ctrl-PgUp/PgDn scroll the diff); files that are sensitive, or
  were produced by a command that read sensitive data, are named and never shown. **Privacy** counts
  what the frontier did not see as it was (results replaced by placeholders, held locally, answered
  by the local model) and lists the interventions, newest first. **Session** shows turns, requests,
  tool calls, tokens, cost and working time against the budgets. Colour unless `NO_COLOR` is set.
- **Talking to duet.** Every message is a turn. duet ends it with `duet:` (a reply), `duet asks:`
  (a clarifying question: your next message is the answer) or `duet finished:` (it called `finish`
  and `checks.commands` passed). `//text` sends a message that starts with `/`. `@path` names a
  file of the workspace (Tab completes it). The input box stays editable the whole time:

  | Keys | |
  |---|---|
  | Enter | send (a line ending with `\` continues instead) |
  | Alt-Enter, Shift-Enter, Ctrl-J | a new line in the message (Shift-Enter where the terminal reports it: kitty, WezTerm, Ghostty, foot, recent iTerm2) |
  | ← → , Ctrl-B / Ctrl-F, Home / End, Ctrl-A / Ctrl-E | move; Alt-B / Alt-F, Ctrl-← / Ctrl-→ by word |
  | Backspace, Delete, Ctrl-W, Alt-Backspace, Alt-D, Ctrl-K, Ctrl-U, Ctrl-Y | delete; cut a word or to the line's end/start; put back what was cut |
  | ↑ ↓, Ctrl-P / Ctrl-N | move between the lines of a message, then through your earlier messages of this session |
  | Ctrl-R | search those messages (type to narrow, Ctrl-R for an older match, Enter takes it into the input, Esc cancels) |
  | Tab | complete a command, an `@path`, or a path after `/image` (several choices show in a box above the input) |
  | PgUp / PgDn, mouse wheel | scroll the conversation |
  | Ctrl-T | the side panel: Changes, Privacy, Session, closed |
  | Ctrl-↑ / Ctrl-↓, Ctrl-PgUp / Ctrl-PgDn | pick a changed file; scroll its diff |
  | Ctrl-O | the settings (below) |
  | Ctrl-Z, Ctrl-D | suspend (`fg` returns); on an empty input, leave (like the end of input) |

  History is the session's own messages, read from its transcript: nothing new is stored. Ctrl-C
  follows the rules below whatever you were typing (the line is cleared). The mouse wheel is the
  workspace's; to select text with the mouse, hold Option (macOS) or Shift while dragging. Anything
  else in the process prints (a warning, a setup note) appears in the conversation, dimmed and
  marked `│`, never over the screen.
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
  next message; see Images below), `/quit` (leave; the session stays open), `/close` (end it for
  good), `/help`; and the settings screens: `/settings`, `/models`, `/sensitivity`, `/ip`,
  `/limits`, `/data`, `/audit`, `/runs`.
- **Resuming.** `/quit`, Ctrl-D or Ctrl-C twice at the prompt leaves the session open; `duet
  --resume` continues the latest open one (with a recap of its last turns), `--resume <id>` a given
  one, also after a crash.
- **Privacy.** You see real values in your terminal; the frontier gets your messages the way it
  gets task text: detected secrets and personal data become placeholders, and values it has seen
  as placeholders stay placeholders. duet's replies show the real values back to you. See
  [SECURITY.md](../SECURITY.md) (Operator messages).
- **Limits.** Each turn is held to `limits.frontier_usd` and `limits.wall_clock_minutes`; the
  session to `session.frontier_usd` and `session.wall_clock_minutes` (time duet works; time
  waiting for you does not count). A turn stopped by a limit leaves the session open; a spent
  session budget ends it until the budget is raised. `oversight.approve` asks in the input box
  (`approve? y/n`); with approval on, a session needs a terminal.
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

**Settings, inside the workspace.** Ctrl-O or `/settings` (or `/models`, `/sensitivity`, `/ip`,
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

**Operator approval** (`oversight.approve`, owner config only; default `off`): with `risky`, Duet
asks y/N on the terminal before a `sensitive_data` command, a protected edit, or a write to anything
other than an ordinary source or test file; with `all`, before every command and write. A refusal
is returned to the model as a tool error, and every decision is in the run's audit log. A run with
approval on and no terminal refuses to start. Details: [SECURITY.md](../SECURITY.md) (Oversight).

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

Other backends are used only when the owner names them in `web.search.backend`; `auto` never picks
them (earlier versions picked Z.ai for a Z.ai frontier, a SearXNG instance whose URL was set, or
Brave when its key was set; runs and `duet doctor` now name those settings as unused):

| Backend | Who receives the queries | Cost |
|---|---|---|
| `native` (`auto`) | each source asked (above) | none, no key |
| `searxng` | your instance at `web.search.searxng_url`, which passes queries on to the engines it is set up with | none |
| `brave` | Brave (key in `$BRAVE_API_KEY`) | Brave's plan |
| `wikipedia` | the Wikimedia Foundation (English Wikipedia's search only) | none, no key |
| `zai` | Z.ai (with a Z.ai frontier, the provider that already receives the run) | coding plan: the plan's search server, counted in the plan's credits; otherwise the Web Search API, billed per search to the account balance (`web.search.zai_engine`) |

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
results are scanned like public content and shown as untrusted data. Every call is an audit event
(host, bytes, outcome; a native search one per source asked). Native sources are held to the same
address rules as `web_fetch`. Details: [SECURITY.md](../SECURITY.md) (Web tools).

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
[SECURITY.md](../SECURITY.md) (MCP servers).

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

**Getting a local model.** With no `local.base_url` in your user config, `duet run` looks for a
server on this machine only (127.0.0.1 on the preset ports 11434, 1234, 8080 and 8000, or
`DUET_LOCAL_PORTS`), lists what answered, and uses it for that run when exactly one model is on
offer (saying so, and recording it in the run). With several, or none, it prints the exact
`duet config set` commands and stops. It never writes configuration: `duet config preset <name>`
does that, through the same `--confirm` and audit path as any endpoint change.

**Privacy mode without a local model.** `duet config set local.enabled false` (a repository's own
config may set it too) runs hybrid mode with no local model: nothing is probed or contacted, no
model reads sensitive content, and the frontier sees it only as handles (their error lines with
values replaced, and line shapes or structure views) and synthetic samples. `ask_local` and `edit_protected` are refused, and the task tells
the frontier so. Local-only mode, and `sensitivity.local_pii_pass` (which would otherwise be skipped
without a word), refuse to start. Turning it back on lets a local model read sensitive content
again, so it needs `--confirm`. The work is harder for the frontier without answers about the
data; the evaluation lane `duet-hybrid-nolocal` measures by how much.

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

**`duet doctor`** checks the configuration and its origins, the config audit chain, settings looser
than their defaults, the frontier endpoint and whether its key variable is set (the value is never
printed), local-endpoint trust (loopback, allowlist, the plain-HTTP rule), the sandbox, git (and,
outside a repository, what works without one), the project instructions found (`DUET.md`), disk
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
  yet. `omlx-coding` does not read images (the vision check found the server drops image parts),
  so with it `local.vision` stays off and images in hybrid runs go to the frontier only when public.

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
