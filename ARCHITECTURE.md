# Duet v2 Architecture

Status: implemented through milestone M4.5, SbD-1 and the SbD-2 tests, plus M5 preparation and
parts of M6 ([docs/PLAN.md](docs/PLAN.md) §10), except where marked *planned*. `duet-tui` (M6) is
built. Goals and success criteria:
[docs/TARGET_STATE.md](docs/TARGET_STATE.md).

## 1. Shape of the system

Duet runs one coding task as one continuous conversation with a **frontier model**. The frontier
decides every step and writes all code in Open paths. A **local model** (loopback or an
owner-allowlisted host) is the only model that ever reads sensitive content; it writes digests and
briefs, answers questions and rewrites protected files on request, has no tools and never decides.
Between the two sits the **boundary**, and between Duet and the network sits a single **outbound
gate**.

```
                      ┌─────────────────────────────────────────────┐
                      │                duet-agent                   │
  task ─────────────► │  Loop ── ToolRegistry ── ContextManager     │
                      │   │            │               │            │
                      │   │      tool results          │ masking    │
                      │   │            ▼               │            │
                      │   │     ┌──────────────────────┴────────┐   │
                      │   │     │         duet-boundary          │   │
                      │   │     │ Engine: classify → view        │   │
                      │   │     │ Vault · Handles · OverlapIndex │◄──┼── LocalReader (local model)
                      │   │     └──────────────┬─────────────────┘   │
                      │   ▼                    │                     │
                      │ GatedFrontier ◄── OutboundGate ── AuditLog   │
                      └─────┬───────────────────────────────────────┘
                            ▼
                     frontier provider (network)

  supporting crates: duet-provider · duet-fs · duet-sandbox · duet-git · duet-config
  front ends:        duet-cli · duet-tui          evaluation: duet-evals (duet-eval)
```

## 2. Crates and dependencies

```
duet-provider    duet-fs    duet-sandbox                  (leaf crates)
duet-git, duet-config        → duet-fs
duet-boundary                → duet-provider, duet-fs
duet-agent                   → duet-boundary, duet-fs, duet-sandbox, duet-git   (not duet-provider)
duet-cli                     → all of the above (composition root)

duet-evals links no duet crate: it drives the duet binary as a black box and has its own
pricing and usage parsing. duet-tui → duet-config, duet-boundary, duet-agent, duet-git (and
ratatui); duet-cli links it for `duet tui` and supplies `duet doctor` as a callback.
duet-release (release tooling: the `duet-sbom` SBOM generator) links no duet crate.
```

| Crate | Owns | Must never |
|---|---|---|
| `duet-provider` | Chat Completions (Responses and Anthropic Messages *planned*, M6), streaming assembly, retry, credentials, local-endpoint trust, context probes, `Usage`, `Price` | Know about tools, policy or the boundary |
| `duet-fs` | `PinnedParent` handle-relative I/O, atomic durable writes, private (0600) files, workspace lock, `.duet` path registry | Open a workspace path by string after validation |
| `duet-sandbox` | Seatbelt/bwrap profiles (write and deny-read lists), env allowlist, output cap with spill file, process-tree capture and kill | Decide what a command is allowed to mean (no refusal logic) |
| `duet-git` | Private checkpoint store; the only function that spawns `git` | Inherit the user's git config, hooks or fsmonitor |
| `duet-config` | Settings registry, file loading, scope and tighten-only rules | Accept owner-only keys from a project file |
| `duet-boundary` | Classification, transformation, vault, handles, bulky offload, IP levels, local roles, local micro-eval, outbound gate, audit | Expose a way to reach the frontier without the gate |
| `duet-agent` | Loop, tools, transcript, context manager, termination, cost ledger, operator approval (`oversight`), disclosure report | Construct a frontier provider (it receives `GatedFrontier`) |
| `duet-cli` / `duet-tui` | User interfaces over config, runs and audits; the CLI is the only place providers are built | Contain policy logic (they edit the registry) |
| `duet-evals` | Tasks, canaries, leak proxy, judge, statistics, reports | Share code paths with the product's privacy decisions |
| `duet-release` | CycloneDX SBOM from `cargo metadata` (offline); used by `tools/release.sh` | Be linked by the product |

## 3. Key types

```rust
// duet-provider
struct ChatProvider;   async fn create(&self, req: &Request) -> Result<Response, ProviderError>
struct Usage { input, cache_read, cache_write, output, reasoning: u64, status: UsageStatus }
enum UsageStatus { Reported, Estimated, Unknown }

// duet-boundary
enum Source { File { path, ranged }, FileList, Search { pattern }, Command { command, exit_code },
              SensitiveCommand { command, exit_code }, Diff, Checks, Other { label } }
enum ViewClass { Raw, Tokenized, HandleSummary, LocalAnswer, BulkyHandle, Protected }
trait Presenter { fn present(&self, &Source, &[u8]) -> String;   // what the frontier gets
                  fn extra_tools(&self) -> Vec<ToolSpec>; fn call_tool(..); fn resolve_for_write(..);
                  fn hidden_from_commands(..) -> Vec<PathBuf>; fn mark_sensitive(..); .. }
struct Engine;         // the hybrid Presenter; PassThrough is the no-op one
// handles render as "h12"; placeholders as "⟨secret:DB_URL#1⟩", "⟨email:email#4⟩", "⟨body:h7⟩"
struct OutboundGate;   fn wrap(self, p: ChatProvider) -> GatedFrontier
struct GatedFrontier;  // only type the agent can call the frontier through

// duet-agent
fn run(cfg, frontier: &GatedFrontier, presenter: &dyn Presenter, git, resume, interrupted)
    -> (Terminal, RunStats)
enum Terminal { Completed { summary }, Failed { reason }, BudgetStopped { which } }
```

## 4. Lifecycle of a run and of one turn

```
0. Run start (hybrid): prime the engine. Public files and the task seed the public-word list;
   every sensitive file (git ls-files, ≤2 MB) is read once: its values enter the vault and its
   text the copied-span index. The task gets a note naming the sensitive paths (commands cannot
   read them) and, with `sensitivity.local_brief` (off by default; it raised cost in Gate 3), the local model's brief of those files for the
   task (≤3 local calls, values withheld, cleaned like any local output).
1. ContextManager builds the request: fixed system prompt + fixed sorted tools + transcript
   (whole old turns replaced by stubs once over `context.mask_at` of the window).
2. GatedFrontier.create(request)
     OutboundGate filters: sanitize every item, including the frontier's own messages, reasoning
       and tool-call arguments (known values → placeholders, detectors) → copied-span filter →
       protected-code redaction
     check: no vault value anywhere in the body; a hit blocks the send and ends the run (fail-closed)
     → append AuditLog record → Provider.create
3. Response parsed. `length`: no tool call executes; the frontier is told to continue in smaller
   steps. `content_filter`: Failed.
4. For each tool call, in order:
     a. validate arguments against the tool schema (errors return as tool results)
     b. write tools: resolve placeholders (secret-sink rule); record values the frontier wrote
     c. execute (duet-fs / duet-sandbox with sensitive paths denied / duet-git)
     d. Presenter.present(source, bytes) → the text the frontier sees
     e. append call + result to Transcript (synced)
5. Loop until finish passes the checks, a budget stops, or a failure a retry cannot fix (§10).
```

## 5. Boundary internals

### 5.1 Classification

Layers run independently; the result is the most restrictive class any layer assigns.

| Layer | Input | Signals |
|---|---|---|
| Path | `Source::File` path | `sensitivity.globs`, `sensitivity.protected_paths`; `.env`-style files are secret-bearing |
| Source | command argv | command output sensitive unless on `sensitivity.raw_ok_commands`; `sensitive_data` output always sensitive |
| Secret detector | any text | provider key formats, JWT, private keys, URL passwords, credential assignments, entropy near key-like names |
| PII detector | any text | email (reserved example domains skipped), phone, card (Luhn), national IDs, IBAN, IP |
| Sensitive-text detector | sensitive text only | title-case name runs (split on stop words), person/address field values of any shape, long numbers |
| Taint | files | files a `sensitive_data` command created or changed (`derived.json`, kept across resume) |
| IP | path | `ip.interface_only` / `ip.sealed` |

Every value found enters the run's **vault** under one token per value (`⟨kind:label#n⟩`). Values
from sensitive text also register **aliases** for their other spellings (a surname alone; a
number grouped, ungrouped or as minor units) unless the spelling is a word of the public files or
the task; aliases map to the same token and are never written back. Values the frontier wrote
itself (test data) are shown as written unless the same value is in the vault.

### 5.2 Transformation

| Class | View body |
|---|---|
| Raw (public) | the content, with detected and known values replaced and copied sensitive spans removed |
| Tokenized (secret-bearing files) | structure intact, every value → placeholder |
| HandleSummary (sensitive files, large command output, `sensitive_data` output) | handle + size + up to 12 error lines (sanitized) + repeated line shapes for texts over 40 lines (digits as `#`, values as `⟨…⟩`) + local summary and facts |
| Command output ≤ 6,000 chars (not allowlisted) | shown sanitized, no local call |
| BulkyHandle (public) | handle + first 40 lines + deterministic outline (declarations; error/warning lines and last 20 lines of output; counts per directory/file for listings and searches) + local summary of command output only (never of source); `read_raw` ranges (≤500 lines, default 200, sanitized) |
| Protected (Interface-only) | tree-sitter skeleton; bodies → `⟨body:hN⟩` |
| Protected (Sealed) | existence only |

Offload thresholds: a file read without a range is shown whole up to `sensitivity.bulky_file_tokens`
(12,000); a ranged read up to 500 lines; allowlisted command output, listings and searches up to
`sensitivity.bulky_tokens` (2,000). Tokens are estimated as characters / 3.

Metadata: sensitive paths are listed by name (and named in the task note); a search reports the
location of a match in a sensitive file but not the line; `diff` is the working-tree diff, sanitized,
with protected files' sections replaced by a note. Git commands are ordinary commands: their
output follows the command-output rules above.

### 5.3 Local roles

The local model is called only by the boundary, never by the loop directly, and has no tools.

| Role | Trigger | Required fields |
|---|---|---|
| Brief | run start, sensitive files other than secret-bearing ones (≤20K chars each, ≤3 calls) | `summary`, `facts[]` |
| Digest | new HandleSummary, or BulkyHandle of command output | `summary` (≤800 chars per chunk), `facts[]` |
| Answer | `ask_local(handle, questions[≤6])`, one call per question on the most relevant chunk | `answer` (≤1,200 chars), `evidence_lines[]`, `unanswerable` |
| Implement | `edit_protected(path, spec, tests?, command?)` | `code`: the whole new protected file, written by the host and validated by host-run checks |

All roles send one shared JSON schema (servers such as oMLX key their prompt cache by schema) and
put the content before the instruction, so questions about one handle reuse the processed prefix;
each role checks its own required fields. Temperature 0; visible thinking off; content over ~60K
characters is chunked; one retry on unparseable output, then an error the frontier sees as
"unavailable" — never an invented answer. Every local output is sanitized as sensitive text and
passes the copied-span filter before it enters a view.

### 5.4 Outbound gate

The gate applies filters to the request, then checks, then appends to the audit log and sends.

- **Filters:** every item is sanitized again (idempotent), including the frontier's own text,
  reasoning and tool-call arguments; copied-span filter; protected-code redaction.
- **Copied-span index:** hashes of 8-token windows of every sensitive text; ≥3 consecutive
  matching windows (~24 tokens) not also present in public files are replaced.
- **Final check:** no vault value (≥6 characters, raw or JSON-escaped) may appear anywhere in the
  serialized body. A hit blocks the send (`BlockedSend` event) and the run fails; nothing is sent.
- **Audit record:** `{seq, prev, unix_ms, endpoint, model, request_sha256, request (after
  substitution), interventions[]}`; `duet audit verify` recomputes the chain.
- **Audit events** share the chain: run start (boundary on/off) and end, local-endpoint trust,
  sandbox denials, `sensitive_data` commands (command, exit code, files marked derived), blocked
  sends (check name), protected edits. Names, paths and outcomes only, never content.
- **Anchor:** after every append the chain head (record count, last hash) is written to
  `<owner state>/audit-anchors/runs/<run-id>/<first-record hash>.json`, outside the workspace.
  The key belongs to the run, not to the workspace path, so a moved or re-mounted workspace still
  verifies; anchors in the earlier layout (`audit-anchors/<workspace hash>/<run-id>.json`) are still
  read. `duet audit verify` reports a log rewritten or truncated since, and a run refuses to
  continue such a log.

Canaries are not known to Duet (they carry no marker); the evaluation's leak proxy finds them.

### 5.5 Write-back

Write tools receive frontier text containing placeholders. A placeholder resolves locally only in
a secret sink (`sensitivity.secret_sinks`) or a sensitive file. Anywhere else, and for any unknown
placeholder, the write is refused with an error telling the frontier to read the value at runtime
(for example from an environment variable of that name). `edit_file` anchors may contain
placeholders; they are matched against the real text.

## 6. Context management

- Transcript is append-only and is the source of every request, so the provider prefix stays
  stable across turns.
- Masking decisions use a size estimate of the request; costs use the provider's reported usage
  (estimated when a server reports none, and counted separately when unknown).
- Above `context.mask_at` (0.7) of `context.window_tokens` (200K), tool results are replaced at once, whole turns at a time
  (oldest first, the last 4 turns kept, results under 400 characters kept), by stubs naming the
  call and any handle: `[masked: run_command `cargo test` → h7, ~3100 tokens removed to save
  context; h7 is still available (...)]`. Tool calls and their results are never separated.
  Masking happens rarely and in batches so caches are invalidated rarely.
- The run's cost ledger (`summary.json` `stats.ledger`) charges every tool result, for each
  request that carries it, to the class it was shown as (raw, tokenized, handle summary, local
  answer, bulky handle), and counts `ask_local` calls and questions, `sensitive_data` commands,
  sandbox denials and local busy seconds; the method is documented in `duet-agent/src/ledger.rs`.

## 7. State on disk

All Duet state lives under the workspace's `.duet/`, classified by the path registry (ignored by
git; reset behaviour defined per entry).

```
.duet/
  config.toml                 project settings (tighten-only)
  git/                        private checkpoint store (bare, fixed config, flock)
  runs/<run-id>/              mode 0700, files 0600; `duet purge` after data.retention_days
    run.json                  manifest (mode, task, frontier) for `duet resume`
    transcript.jsonl          full conversation items, synced per item
    handles/<hN>(.source)     raw bytes of handles (local only)
    vault.json                placeholder ↔ value map, aliases (local only)
    derived.json              files made sensitive by `sensitive_data` commands
    spill-<uuid>.txt          long command outputs
    writes.jsonl              pending/applied records for crash recovery
    summary.json              terminal state, usage, cost ledger
  audit/<run-id>.jsonl        hash-chained outbound log (placeholder-substituted)
  lock, tmp/                  workspace lock; sandbox scratch space
~/.config/duet/config.toml    owner settings (credentials, endpoints, local address, policy)
~/.local/state/duet/          owner state ($DUET_CONFIG_HOME/state when set), mode 0700
  audit-anchors/runs/<run>/<first>.json   chain head of each run's audit log
  config-audit.jsonl          hash-chained log of `duet config set` changes
```

## 8. Safety primitives

- **File I/O:** all workspace access goes through `PinnedParent` (`O_NOFOLLOW` walk, `openat`,
  `renameat`, `fsync`); writes are atomic with digest preconditions and rollback; `.git` and
  `.duet` are never writable by tools.
- **Commands:** Seatbelt (macOS) or bwrap (Linux) with absolute binary paths; workspace-write with
  `.git`/`.duet` unwritable, and unreadable to ordinary commands and checks (committed copies, the
  vault); command `TMPDIR` outside the workspace; sensitive and protected paths unreadable (deny-read) except for
  `sensitive_data` commands, and protected source readable by the host's checks; network off
  unless allowed; tmpfs `/run`; restricted service lookup; process-tree kill on timeout or interrupt.
- **Git:** one helper; environment cleared; fsmonitor, hooks, filters and user/system config
  disabled.
- **Credentials:** the owner config names an environment variable (`frontier.api_key_env`,
  `local.api_key_env`); the value is read at request time and never written to config, transcripts
  or audit.

## 9. Configuration

`duet-config` holds a registry of every setting:

```rust
struct Setting { key, kind: Bool | Int{min,max} | Float{min,max} | Str | List | Choice,
                 default, scope: Owner | Project,
                 direction: Any | AddOnly | OnlyTrue | OnlyFalse | OnlyLower,
                 confirm: bool, help }
```

Loading merges defaults → owner file → project file, rejecting owner-only keys in project files
and any project value that loosens privacy; reading an unregistered key is an error. `duet config
list|get|set` and `duet tui` are driven by the registry and share `Config::propose` / `apply`. `duet config set` refuses a
change that loosens privacy (a `confirm` setting, or a value against its direction) without
`--confirm`, prints the diff and appends every applied change to the owner's `config-audit.jsonl`.

## 10. Termination and recovery

Every run ends in exactly one `Terminal` state, with `summary.json` and the audit log's `run_end`
event (which anchors the log's final head) written by `duet_agent::conclude`:

- `Completed{summary}`: `finish` passed the checks.
- `Failed{reason}`: the task cannot be completed as things stand: an error a retry cannot fix
  (credentials, an invalid request, a context overflow), a send blocked by the outbound gate,
  `content_filter`, repeated `length` stops or text-only turns, checks still failing after
  `limits.max_finish_attempts`, an interrupt (`interrupted; resume with duet resume`), or a
  panic anywhere in the run (`internal error: <message>`). `duet_agent::run` catches panics in the
  loop, tools, engine and gate; the CLI also catches errors and panics in run setup, so once a
  run directory exists it always gets a summary.
- `BudgetStopped{which}`: `frontier_usd` (checked before each turn) or `wall_clock`.

Infrastructure failures retry in place: frontier and local-model connect errors, timeouts, stream
cuts, 429 and any 5xx retry with jittered exponential backoff (capped at 60 s) and no attempt cap.
The run's wall clock is one deadline shared by the loop and both providers; an outage that outlasts
it ends the run as `BudgetStopped{wall_clock}`, and an attempt in flight is cut off at it. A tool
result produced after the deadline or an interrupt (for example a local summary that could not be
made) is not recorded; the turn is re-decided on resume.

- `length` / `content_filter` stops never execute tools.
- Writes record `pending` before and `applied` after; `duet resume <run>` reconciles pending writes,
  drops a partly answered turn and continues the transcript and the same audit chain. It refuses a
  completed run and a malformed run id.
- Ctrl-C stops a frontier wait or a retry at once, kills a running command's whole process tree
  (checks included), records the interrupted call in the transcript (its result is not recorded)
  and ends in resumable `Failed{interrupted}`.

A full disk is an infrastructure failure too (`duet_agent::host`). A write of run state that fails
for lack of a host resource (disk space, quota, file handles; `FsError::is_host_resource`) pauses
the run: one notice on stderr (`disk full: waiting for space`), then the write is retried in place
with backoff (0.25 s doubling, capped at 30 s) until it succeeds, the run is interrupted, or the wall
clock ends it as `BudgetStopped{wall_clock}`. This covers the transcript, the write journal (each
step on its own), workspace writes and the audit log (the line, then its anchor); the records
that end a run (the transcript's end entry, `summary.json`, the audit
`run_end`) keep waiting for up to 30 s past the deadline. Every such write is all or nothing: a
failed append is cut back to its previous length and a failed whole-file write removes its
temporary file, so a retry never duplicates or tears a record.

The estimated usage of frontier attempts that failed after output started is charged like billed
usage, kept apart from it: to the dollar budget and `cost_usd`, to the ledger
(`failed_attempts_usd`, included in `input_usd`/`output_usd`) and to the transcript
(`failed_attempts` entries, replayed on resume); `stats.failed_attempt_usage` holds the tokens.

## 11. Evaluation architecture

`duet-eval` treats every agent — Duet in its three modes and external agents — as a black box
behind a **logging reverse proxy** that sits between the agent and its frontier endpoint.

```
task workspace (with canaries) ──► agent lane ──► leakproxy ──► frontier API
                                                     │
                                                leaks.jsonl  (independent of Duet's audit log)
workspace ──► sealed grader (hidden tests, secret-sink scan) ──► judge (code quality) ──► stats ──► report
```

Tasks come from the tiered dogfood suite (docs/DOGFOOD_SUITE.md). Gates use paired designs:
quality passes when the judge score is non-inferior (lower bound of the paired difference > −2/30)
and the candidate is not behind on hidden-test pass rate on a majority of tasks; cost needs the
upper bound of the paired difference < 0; privacy needs zero leaks. Duet runs write their cost
ledger to `summary.json`, which `duet-eval report` reads per lane.

## 12. Invariants

Each is backed by a test, except where noted.

1. No code path reaches the network with a frontier request except through `OutboundGate`.
2. No vault value, canary or ≥24-token span of a sensitive handle appears in any audit record.
3. The local provider's endpoint is loopback or owner-allowlisted, and a remote one uses TLS unless
   the owner set `local.allow_plaintext`.
4. Project configuration cannot loosen privacy or set owner-only keys.
5. The system prompt and tool list are byte-identical for every turn of a run.
6. A tool call is never persisted or sent without its result.
7. Every run ends in a `Terminal` state, with a summary and an audit end event, also on a panic;
   no run exits with pending writes unrecorded.
8. `.git` and `.duet` are not writable by tools or sandboxed commands.
9. No file under `crates/` contains another coding agent's code, format or name (outside eval lane
   adapters).
