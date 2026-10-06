# Declass architecture

This reference describes the implementation, component boundaries and extension
interfaces. Start with the [documentation index](docs/README.md) for user guides,
[security design](docs/SECURE_BY_DESIGN.md) for the protection mechanisms, and
[publication readiness](docs/PUBLISH_READINESS.md) for measured coverage and remaining
work. [Historical goals and reviews](docs/evidence/README.md) retain the original
criteria and earlier findings.

## 1. Shape of the system

Declass runs one coding task as one continuous conversation with a **frontier model**. The frontier
decides every step and writes all code in Open paths. A **local model** (loopback or an
owner-allowlisted host) is the only model that ever reads sensitive content; it writes digests and
briefs, answers questions and rewrites protected files on request, has no tools and never decides.
Between the two sits the **boundary**, and between Declass and the network sits a single **outbound
gate** for the frontier and a single **checked client** (`declass-net`) for every other party (§5.6).

```
                      ┌─────────────────────────────────────────────┐
                      │                declass-agent                   │
  task ─────────────► │  Loop ── ToolRegistry ── ContextManager     │
                      │   │            │               │            │
                      │   │      tool results          │ masking    │
                      │   │            ▼               │            │
                      │   │     ┌──────────────────────┴────────┐   │
                      │   │     │         declass-boundary          │   │
                      │   │     │ Engine: classify → view        │   │
                      │   │     │ Vault · Handles · OverlapIndex │◄──┼── LocalReader (local model)
                      │   │     └──────────────┬─────────────────┘   │
                      │   ▼                    │                     │
                      │ GatedFrontier ◄── OutboundGate ── AuditLog   │
                      └─────┬───────────────────────────────────────┘
                            ▼
                     frontier provider (network)

  supporting crates: declass-provider · declass-fs · declass-sandbox · declass-git · declass-config
  front ends:        declass-cli · declass-tui          evaluation: declass-evals (declass-eval)
```

### Security review services

`declass-agent::review` owns a private, revision-pinned run-start snapshot and compares it with a
bounded finish-time snapshot. `declass-review` supplies deterministic candidates, imports and
same-file helper context. Optional LSP references are confined to the workspace and read back
from the pinned snapshot. `declass-cli::scan` invokes the same engine with an empty baseline for
whole-repository review, in the foreground or a detached worker, and stores commit/dirty metadata.

`review::external` runs owner-installed tools in a read-only snapshot sandbox with no network.
It parses checked locations from bounded JSON, retaining raw evidence only in private artifacts.
Every candidate survives model disagreement. Only narrowly rule-confirmed high findings can block.
The boundary supplies known private-value ranges, hosts tool-free local judgment and checks
eligibility for `SecondReviewer`. The latter has a separate gated provider, bounded diff/context,
no tools/history, an output allowance, deadline and dollar reservation. Review responses are
filtered as exploration of every source consulted. Count-only audit events record the result.

`ReviewUsage` transcript entries restore secondary-provider tokens, failed-attempt estimates and
cost after resume without adding reviewer prompts to the working conversation. `Ledger::review`
is a breakdown of spend already included in the total. Local review uses the existing local
usage counters. The defaults and measured limits are in the
[auditor report](docs/evidence/reviews/security-auditor-2026-09-30.md).

## 2. Crates and dependencies

Runtime token prices come from `declass-provider::catalog`: exact OpenRouter slugs or unique
unqualified IDs, cache rates and conditional context/UTC tiers. The CLI saves the public catalog
for 24 hours, then refreshes it; offline/failure fallbacks identify their source and age. Unknown
frontier prices stop before a model request. `pricing.frontier_model` explicitly maps endpoint
aliases. The frozen `price::builtin` table remains for historical evaluation reproducibility.
Local providers share `meter::Meter` across their roles, using owner-configured input/output
rates (zero by default). The CLI persists cumulative local costs and price history separately
from frontier budget accounting; the TUI reads those reports and transcript costs.

```
declass-provider    declass-fs    declass-governor    declass-review  (leaf crates)
declass-sandbox                 → declass-governor   (every command's and server's tree held to memory limits)
declass-git, declass-config        → declass-fs
declass-extensions              → declass-fs   (bounded portable-skill catalog; no execution or network)
declass-boundary                → declass-provider, declass-fs, declass-review
declass-net                     → declass-boundary   (the one HTTP client and resolver for third parties; reqwest)
declass-mcp                     → declass-boundary, declass-net   (MCP client, JSON-RPC 2.0 over stdio and streamable HTTP)
declass-web                     → declass-boundary, declass-net, declass-mcp   (host-side HTTP for the web tools; url, ipnet)
declass-lsp                     → declass-sandbox   (language-server client; tokio, url)
declass-egress                  → declass-boundary, declass-sandbox, declass-web   (host-side egress proxy for commands; tokio, url)
declass-agent                   → declass-boundary, declass-review, declass-config, declass-extensions, declass-fs, declass-sandbox, declass-git, declass-web, declass-mcp, declass-lsp, declass-egress   (not declass-provider)
declass-cli                     → all of the above (composition root)

declass-evals links no declass crate but declass-governor (a leaf that watches process trees and knows
nothing of the boundary): it drives the declass binary as a black box and has its own
pricing and usage parsing (its tests check that table against declass-provider's prices and its
lanes' owner configs with declass-config's loader). declass-tui → declass-config, declass-boundary, declass-agent, declass-git (and
ratatui): everything the operator sees on a terminal (the workspace, `term`, the settings
screens); declass-cli runs the session and supplies `declass doctor`, detection and the cache probe to the
settings as callbacks.
declass-release (release tooling: the `declass-sbom` SBOM generator) links no declass crate.
```

| Crate | Owns | Must never |
|---|---|---|
| `declass-review` | Bounded syntax rules, same-file context, host/guard inventory and before/after comparison shared by the auditor and repository scanner ([scope and measurements](docs/evidence/reviews/security-auditor-2026-09-30.md)) | Perform I/O, execute source, contact a model, or turn a model opinion into a blocking decision |
| `declass-provider` | Chat Completions (Responses and Anthropic Messages *planned*, M6), streaming assembly (and a read-only tap on it, `live`), retry, credentials, local-endpoint trust, context probes, `Usage`, `Price`; images (`image`: decode, scale and re-encode PNG/JPEG/GIF/WebP, each dialect's wire form, digest redaction for audit, the vision probe) | Know about tools, policy or the boundary |
| `declass-fs` | `PinnedParent` handle-relative I/O, atomic durable writes, private (0600) files, workspace lock, `.declass` path registry | Open a workspace path by string after validation |
| `declass-governor` | Memory limits for a process tree: follows the tree from its root (id plus start time), reads the physical footprint, stops a process above the per-process limit, the largest above the tree limit or under critical machine pressure, and every member left when the watch ends; the configured limits (`set_limits`) | Signal a process outside the tree it watches |
| `declass-sandbox` | Seatbelt/bwrap profiles (write and deny-read lists), network modes (`Network`: off, the egress proxy's route, all) and the bubblewrap bridge to the proxy (`bridge`), the list of credential stores in the home directory (`HOME_SECRETS`), env allowlist, output cap with spill file, process-tree capture and kill | Decide what a command is allowed to mean (no refusal logic) |
| `declass-egress` | The egress proxy commands reach package registries through: `CONNECT` and plain-HTTP `GET`/`HEAD`, host allowlist (`Hosts`), its own name resolution with the web tools' address classes, the TLS server-name check, each plain request and tunnel host checked by the run's guard, one route per command, one event per connection | Look inside a TLS tunnel, or decide which command gets network |
| `declass-net` | The product's one HTTP client and resolver for third parties: sends only a `declass_boundary::third_party::Checked` request (no redirects, no proxy from the environment, a checked address pinned), adds owner credentials after the check, resolves only a checked request's host | Build a request itself, or send anything unchecked |
| `declass-git` | Private checkpoint store; the only function that spawns `git`; plumbing-only commits of given paths (`commit_paths`), operator identity, commit blockers; file listing (git, or outside a repository a walk honouring `.gitignore`, `walk`) | Inherit the user's git config, hooks or fsmonitor |
| `declass-web` | Every request checked by the caller's `Guard` and sent through `declass-net`; guarded `GET` fetch (search-engine result pages refused) (address checks after DNS, connection pinned to the checked address, redirects re-checked, size cap, timeout), HTML to text, search backends: native (`search::native`: public sources with open APIs asked in parallel through the guarded client, paced per service, backoff, answers kept per run, merged), SearXNG, Brave, Wikipedia alone (paced to one request a second), Z.ai (the coding plan's search server through `declass-mcp`, or the Web Search API) | Decide what the frontier sees, or read the workspace |
| `declass-mcp` | MCP client from the public specification: `initialize` with revision negotiation, paginated `tools/list`, `tools/call`, content rendered as text (non-text described), timeouts with cancellation, sessions (`Mcp-Session-Id`), size caps, no redirects; the HTTP transport checks every message with the `Guard` it was opened with and sends through `declass-net`; `ServerConfig`; scripted servers for tests | Start processes, or decide what a server may see or return |
| `declass-lsp` | Language-server client from the LSP 3.17 specification: `Content-Length` framing, requests with timeouts and `$/cancelRequest`, minimal answers to server requests, published diagnostics and work-done progress, per-language servers started lazily in the sandbox (`declass_sandbox::spawn`) and restarted once; built-in server table and `lsp.servers.<language>`; UTF-16 positions; a scripted mock server for tests | Decide what may be sent to a server or shown to the frontier (the caller does) |
| `declass-config` | Settings registry, file loading, scope and tighten-only rules, the policy layer of an embedding program (`policy`, §13) | Accept owner-only keys from a project file, or any value past a loaded policy |
| `declass-extensions` | Portable `SKILL.md` catalog, metadata parsing, bounded no-follow resource reads and instruction digests | Execute code, contact a model/server, or grant invocation permissions |
| `declass-boundary` | Classification, transformation, vault, handles, bulky offload, condensed command output (`condense`), structure views and synthetic samples of sensitive data (`structure`), IP levels, local roles, local micro-eval, outbound gate, the third-party check (`third_party`: `Outgoing`, `Checked`, `Guard`), audit | Expose a way to reach the frontier without the gate, or to make a `Checked` request without the check |
| `declass-agent` | Loop, tools, transcript, context manager (masking, compaction), termination, cost ledger, operator approval (`oversight`), disclosure report, sessions (operator turns, steering, undo), sub-agents (`delegate`), the local explorer (`explore`), root/scoped instructions (`instructions`), skill authorization and filtered loading (`skills`), changes outside git (`changes`) | Construct a frontier provider (it receives `GatedFrontier`) or a local one (the explorer receives a `LocalAgent`) |
| `declass-cli` / `declass-tui` | User interfaces over config, runs and audits; the CLI is the only place providers are built; the operator's terminal (`term`: the chat console, its line editor, Markdown rendering of streamed text, `declass run` progress); `declass-cli` is also a library whose `main_with` is the whole command line for a program that embeds Declass (§13) | Contain policy logic (they edit the registry) |
| `declass-evals` | Tasks, canaries, leak proxy, judge, statistics, reports | Share code paths with the product's privacy decisions |
| `declass-release` | CycloneDX SBOM from `cargo metadata` (offline); used by `tools/release.sh` | Be linked by the product |

## 3. Key types

```rust
// declass-provider
struct ChatProvider;   async fn create(&self, req: &Request) -> Result<Response, ProviderError>
struct Usage { input, cache_read, cache_write, output, reasoning: u64, status: UsageStatus }
enum Item { User { text }, Assistant { .. }, ToolResult { call_id, content },
            Images { call_id: Option<String>, images: Vec<Image> } }   // images of the item before
struct Image { media_type, sha256, width, height, bytes }   // bytes loaded, never serialized
enum UsageStatus { Reported, Estimated, Unknown }

// declass-boundary
enum Source { File { path, ranged }, FileList, Search { pattern }, Command { command, exit_code },
              SensitiveCommand { command, exit_code }, Diff, Checks, Web { url },
              GitHistory { rev, path: Option<PathBuf> }, Other { label },
              Mcp { server, tool, trust: ServerTrust }, Subagent { child }, Image { origin } }
enum ViewClass { Raw, Tokenized, HandleSummary, LocalAnswer, BulkyHandle, Protected }
trait Presenter { fn present(&self, &Source, &[u8]) -> String;   // what the frontier gets
                  fn extra_tools(&self) -> Vec<ToolSpec>; fn call_tool(..); fn resolve_for_write(..);
                  fn hidden_from_commands(..) -> Vec<PathBuf>; fn mark_sensitive(..);
                  fn check_outbound(&self, destination, text) -> Result<String, String>;
                  fn outbound_guard(&self) -> third_party::Guard;   // owned check for third parties
                  fn route_image(&self, &ImageRequest) -> Route; .. }   // Frontier | Describe | Refuse
struct Engine;         // the hybrid Presenter; PassThrough is the no-op one
// handles render as "h12"; placeholders as "⟨secret:DB_URL#1⟩", "⟨email:email#4⟩", "⟨body:h7⟩"
struct OutboundGate;   fn wrap(self, p: ChatProvider) -> GatedFrontier
struct GatedFrontier;  // only type the agent can call the frontier through
// declass-boundary::third_party (third parties: web pages, search sources, MCP over HTTP, the proxy)
struct Outgoing { method, url, headers, body }   // a request before the check
struct Checked;        // made only by Guard::check; the only thing declass-net sends
struct Guard;          // a presenter's check, owned: check(), check_text(), check_value(), name()

// declass-agent
fn run(cfg, frontier: &GatedFrontier, presenter: &dyn Presenter, git, resume, interrupted)
    -> (Terminal, RunStats)
enum Terminal { Completed { summary }, Failed { reason }, BudgetStopped { which } }
trait Driver;          // the model a loop is driven by: model(), deadline(), audit(), create(request);
                       // implemented by GatedFrontier (the run's frontier or subagents.model)
struct Subagents { max_parallel, max_usd, max_time, model: Option<Arc<dyn Driver>>, price }
struct Session;        // open(cfg, frontier, presenter, git, interrupted, limits, resume)
                       // turn(&mut self, message) -> TurnEnd; undo(); steering(); end(closed)
enum TurnEnd { Replied { message }, Asked { question }, Completed { summary }, Failed { reason },
               BudgetStopped { which }, Interrupted, Stopped }
struct Steering;       // steer(message), stop(): the operator's side of a running turn
```

## 4. Lifecycle of a run and of one turn

```
0. Run start (hybrid): prime the engine. Public files and the task seed the public-word list;
   every sensitive file (`Git::list_files`: git ls-files, or outside a repository a walk that
   honours `.gitignore`/`.ignore` (`declass_git::walk`), plus the sensitive files it does not list — an ignored
   `.env`, logs, databases — found by the walk that builds the commands' deny list, ≤64 MB of
   those in total; ≤2 MB per file) is read once: its values enter the vault and its
   text the copied-span index (and, with `sensitivity.structure_views`, its string values the
   masked-output index and its structure an outline). The task gets a note naming the sensitive
   paths (commands cannot read them), the outline of each file the policy makes sensitive (≤4,000
   characters) and, with `sensitivity.local_brief` (off by default; it raised cost in Gate 3), the local model's brief of those files for the
   task (≤3 local calls, values withheld, cleaned like any local output).
   The first message starts with `instructions::block`: owner instruction files sanitized like
   operator messages, then repository-root files presented as `Source::File`, each framed
   between digest-tagged markers, ≤16 KiB per file / ≤64 KiB total, audited as `instructions`.
   Files are AGENTS (or AGENTS.override), CLAUDE, GEMINI, root Copilot, then DECLASS; owner lookup
   excludes Copilot. A short filtered skill catalog follows, then the sanitized task. The
   opening message is in the transcript, so resume replays the original root instructions.
1. ContextManager builds the request: fixed system prompt (`prompt::system_prompt`, from the
   workspace name, the checks and the tool set: its research advice names web tools only when
   they are offered) + fixed sorted tools + transcript
   (with `context.compaction`: older turns condensed by the local model once over
   `context.compact_at`; whole old turns replaced by stubs once over `context.mask_at` of the
   window; §6).
2. GatedFrontier.create(request)
     OutboundGate filters (engine/outbound.rs), two passes: detectors, copied-span filter and
       protected-code redaction on messages and tool results (new values join the vault); then
       every value known by then replaced in every string of the request (system prompt, tool
       descriptions, every field of every item including the frontier's own messages, reasoning
       and tool-call arguments, replayed reasoning, JSON inside strings)
     check: no vault value in any string of the body outside its framing (the strings of the
       body of the same request with all content blanked, gate::framing_request); the filter
       reads a request exactly as the check does (property: tests/outbound_agreement.rs)
       refused → filters withhold each part still holding a value (PART_WITHHELD), check again:
       passes → send_withheld event, sent; still refused → blocked_send, the run fails (fail-closed)
     → append AuditLog record → Provider.create
3. Response parsed. `length`: no tool call executes; the frontier is told to continue in smaller
   steps. `content_filter`: Failed.
4. For each tool call, in order, one after another (a later call may rely on an earlier one having
   run; the frontier sees all their results in the next request; consecutive `delegate` calls start
   their sub-agents together, up to `subagents.max_parallel`):
     a. validate arguments against the tool schema (errors return as tool results)
     b. for read_file/edit_file/write_file/edit_protected/rename, discover applicable descendant
        instruction files for the named path. New guidance defers the operation; mutations later
        in that response also wait. The model retries after reading the tool result.
     c. write tools: resolve placeholders (secret-sink rule); record values the frontier wrote
     d. execute (declass-fs / declass-sandbox with sensitive paths denied / declass-git)
     e. Presenter.present(source, bytes) → the text the frontier sees
     f. append call + result to Transcript (synced)
5. Loop until finish passes the checks, a budget stops, or a failure a retry cannot fix (§10).
```

### Sessions (the `declass` workspace)

A session is a run whose conversation continues across operator turns (`declass_agent::session`). It
uses the same loop (`run::work`), tools, presenter, gate and transcript; what differs is how a turn
ends and what enters the conversation between steps.

```
open    new: transcript Start (the first message is the objective the engine was primed with)
        resume: roll back pending writes, rebuild items and usage from the transcript (as
        `declass resume`), recover turn marks, pending notes and working time
turn    reset the stop flag; roll back pending writes; drop a partly answered frontier turn
        TurnStart{exchange, message as typed, journal_next}
        operator item = notes (interrupted turn, undone writes) + message
          first message: Presenter.sanitize_objective (sensitive-path note, brief), after the
                         project instructions (as a run's)
          later ones:    Presenter.sanitize_message (same sanitizer, no notes)
          each placeholder a message introduces maps to a handle holding the message
          (`operator.json`), so ask_local accepts it; the first such message says so once
        audit operator_message{exchange, placeholders}, then the item (the request that
        carries it is audited by the gate as always)
        work() until the turn ends:
          reply / ask_operator tool, or a message without tool calls  -> Replied / Asked
          finish with checks passing                                  -> Completed
          stop requested at the safe point                            -> Stopped
          interrupt, per-turn or session budget, failure              -> Interrupted / BudgetStopped / Failed
        TurnEnd{exchange, seconds, end}: texts with placeholders restored for the operator
end     transcript End: Completed (closed by the operator), Open (left resumable), or
        BudgetStopped{session.*}; the CLI then concludes the run as usual
```

Older session exits stored as `Failed` with the exact `session left open` resume notice
are read as `Open`; their original transcript and audit bytes are retained. Failed and
interrupted turns remain separate entries, even when the session is left open.

**Safe point and steering.** The loop's safe point is the top of each iteration: every result of
the previous response is recorded, the next request is not sent. Steering messages the operator
sent meanwhile (`Steering::steer`) are delivered there, together and in order, as one operator item
(`Steered{exchange, after_request, messages}` then the sanitized `Item`), so a call and its result
are never separated and a running command is never cut short. A stop request (`Steering::stop`)
ends the turn at the same point. Appending is the only change to the conversation: earlier items
are never rewritten, so the provider's prefix cache still applies, and a resumed session replays the
steering in place. A message that arrives after the last request of a turn is handed back
(`Steering::take`) and starts the next turn.

**Limits.** Each turn is bounded by the run limits (`limits.frontier_usd`,
`limits.wall_clock_minutes`) and by what is left of the session's (`session.frontier_usd`,
`session.wall_clock_minutes`, counting only time the agent works); the stop names the binding
setting. The provider's own retry deadline is set far out for a session; each turn cuts its request
off at the turn deadline.

**Undo.** `TurnStart.journal_next` marks where a turn began in the write journal. `Session::undo`
restores every file written by records from that mark on to its content before the first of them
(removing files the turn created), records `Undone{exchange, paths}` and tells the frontier with the
next message. Repeating it walks back turn by turn. Writes made by commands are not journaled and
are not reverted.

**The operator's terminal (`declass_tui::workspace`, `declass_tui::term`).** `declass` without a command
runs the session in `declass-cli` (`chat.rs`: the inbox, steering at safe points, approvals, `/undo`,
budgets) and, on a terminal, draws it with the workspace. Two feeds reach the screen, both read-only:

```
transcript.jsonl ──Follow::drain (every 150 ms, and at each request's start)──▶ Entry ──┐
                                                                                         ▼
provider stream ─Assembler::view─▶ live::Differ ─▶ Ordered tap ─▶ channel ─▶ Feed ─▶ conversation
  (GatedFrontier passes the task-local tap set by      (drains the transcript  Detok, FieldReader,
   declass_boundary::live::observe; sub-agents run quiet)   on Attempt first)      Markdown, words
                                                                                         │
      side panel: Changes (journal + git), Privacy (entries), Session (status) ◀─────────┘
```

The ordered tap reads the transcript to its end when a request starts, so every step recorded
before a response shows before it. One thread owns the screen and the keyboard (crossterm raw keys,
alternate screen, mouse wheel, bracketed paste); it draws with ratatui on `/dev/tty`, and while it
runs the process's standard output and error are a pipe whose lines it shows in the conversation
(dimmed, through `term::safe`), so no print from any crate can corrupt the screen; the terminal and
both descriptors are restored on exit, panic and Ctrl-Alt-Z (Ctrl-Z undoes input edits). The conversation is a list of cells
(`workspace::cells`: the operator's messages, replies rendered as Markdown, tool calls with their
results as the frontier saw them, edits with their diffs, the boundary's interventions, turn ends),
each rendered at the view's width and cached by version; declass's styled text is translated from its
own SGR codes into ratatui spans (`workspace::ansi`; any other escape is dropped) and wrapped at
spaces. The palette, the `@` picker (the workspace's files from `declass_git`), the approval dialog,
help, conversation search and mouse selection are the workspace's own. The command palette sizes
its visible range from the available height, keeps the selected command visible after resizing,
and validates its last-drawn query before using a mouse row. Paging, first/last navigation and
wheel input stay in the menu; a click selects and Enter executes through the normal command path.

The editor (`term::editor`) moves and selects by Unicode grapheme, with bounded input and undo/redo
history. Copy acts on an input or conversation selection before Ctrl-C can interrupt a turn.
Bracketed paste inserts text without submitting; settings paste cannot answer an approval.
The editor sends lines to the same inbox the reader thread fills without a terminal; Ctrl-C with
no selection goes through the same `Interrupts::press` as the signal.

Explicit clipboard actions run on workers (`clipboard`, `workspace::interaction`). macOS uses
fixed AppKit/JXA helpers; Linux uses trusted system Wayland/X11 helpers. Native operations have
bounded pipes and a shared three-second deadline. Reads are disabled over SSH; copy can fall back
to a write-only OSC 52 request, whose receipt the TUI does not assume. A changed draft cancels a
pending paste result. Enter waits for an image preparation job to enqueue its attachment.
The CLI owns private external image snapshots and attachment IDs, with `/attachments` and
`/detach` controls. Images keep their existing routing policy. Background attachment commands
cannot answer approvals; queued image/message groups preserve order, and a failed group recovers
its unsent request instead of attaching its images to a later request.

The side panel's Changes view follows the session's write journal
(`changes::update`) with the engine's own policy, which the CLI hands over when the session opens,
so the panel holds back exactly what the engine holds back. The settings screens (`app`, `ui`: the
registry screens, IP levels, data, audit, the runs view) open as an overlay; their edits go through
`Config::propose` / `apply` as before. `Screen::Plain` keeps the line-by-line output without a
terminal. `declass run` uses the same feed (`declass-cli`'s `term::watch`) on standard error: live on a
terminal, compact lines (placeholders kept, a heartbeat) otherwise; standard output keeps the
summary.

## 5. Boundary internals

### 5.1 Classification

Layers run independently; the result is the most restrictive class any layer assigns.

| Layer | Input | Signals |
|---|---|---|
| Path | `Source::File` path | `sensitivity.globs`, `sensitivity.protected_paths`; `.env`-style files are secret-bearing |
| Source | command argv | command output sensitive unless on `sensitivity.raw_ok_commands`; `sensitive_data` output always sensitive |
| Secret detector | any text | provider key formats, JWT, private keys, URL passwords, credential assignments (`detect.rs`); high-entropy tokens judged by their parts, so paths, hashed build files, identifiers, UUIDs and digests are not keys and a random part in them is (`detect/random.rs`) |
| Imported secret rules | any text (path conditions and path allowlists on file content) | the gitleaks rule set as data (`rules/gitleaks.toml`, compiled by `rules.rs`): keyword prefilter (one Aho-Corasick pass), then each triggered rule's expression, entropy threshold, secret group and allowlists; a match inside an own detector's span is dropped; placeholders named after the rule |
| PII detector | any text | email (reserved example domains skipped), phone (US, E.164, labelled national), card (Luhn), SSN, IBAN (registry length, mod-97), UK/DE/FR/ES/IT/NL national IDs with their checks (`pii.rs`), IPv4, IPv6, labelled postal addresses; 12–19 digit numbers labelled within three words (card, account, IBAN, SSN, passport, licence, tax ID, ...) as card, account or ID whatever their checksum. A regex set per detector group skips expressions that cannot match |
| Local personal-data pass | public content, optional (`sensitivity.local_pii_pass`) | prose lines sent to the local model, which lists names and postal addresses; values that occur as written and look plausible enter the vault (`engine/pii_pass.rs`) |
| Operator-number rule | operator text (task, session and steering messages) | every 12–19 digit number, labelled or not (`⟨id:number#n⟩`, an `ask_local` handle) |
| Sensitive-text detector | sensitive text only | title-case name runs (split on stop words and sentence-initial function words), person/address field values of any shape, long numbers; in local-model output a name counts only if a word of it occurs in the content the model read; digit runs sharing 4+ consecutive digits with a vaulted card, account, ID, IBAN or phone number are replaced |
| Taint | files | files a `sensitive_data` command created or changed (`derived.json`, kept across resume); under `target/` and `node_modules/` (which the snapshot skips) those modified since the command started; each is indexed again, and denied to commands by name |
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
| HandleSummary (sensitive files, large command output, `sensitive_data` output) | handle + size + up to 12 error lines (sanitized) + repeated line shapes for texts over 40 lines (digits as `#`, values as `⟨…⟩`) + local summary and facts. With `sensitivity.structure_views` (§5.12): the structure view (or a log's line templates) in place of the line shapes and, for a data file, the first record of its synthetic sample; for `sensitive_data` output up to 80 lines, the masked output in place of the error lines; short `sensitive_data` output past its budget: a fixed text, no handle |
| Tokenized: synthetic sample (`synthetic_sample`) | the file's records with every value a rule-generated fake, checked before it is shown |
| Command output ≤ 6,000 chars (not allowlisted) | shown sanitized, no local call |
| Condensed (public command and check output, and the ≤ 6,000-char output above, in a recognized format; `context.condense_output`) | handle line + the condensed view (kept lines in order; each omitted run one `[lines a-b omitted: …]` marker; a sum of passing `test result` lines) + `condensed from N lines; read_raw(handle=…)`; the whole output is sanitized first and the public handle keeps that sanitized text; no local call; recorded as BulkyHandle |
| BulkyHandle (public) | handle + first 40 lines + deterministic outline (declarations; error/warning lines and last 20 lines of output; counts per directory/file for listings and searches) + local summary of command output only (never of source); `read_raw` ranges (≤500 lines, default 200, sanitized) |
| Protected (Interface-only) | tree-sitter skeleton; bodies → `⟨body:hN⟩` |
| Protected (Sealed) | existence only |

Offload thresholds: a file read without a range is shown whole up to `sensitivity.bulky_file_tokens`
(12,000); a ranged read up to 500 lines; allowlisted command output, listings and searches up to
`sensitivity.bulky_tokens` (2,000). Tokens are estimated as characters / 3.

Condensing (`condense.rs`, one module per runner family under `condense/`): every line of the output
is marked kept or omitted (and why) by the rules of each format recognized in it (by content: a
format's summary or harness lines, never the command name); lines no rule decides are kept. Kept:
failing tests and their captured output (first and last lines, and between them lines carrying an
assertion, expected/actual or error, up to 20), the first diagnostic of each kind whole up to a cap
and later ones as header, location and labelled source, summaries, exit codes, the sandbox's notes.
Omitted: passing and skipped tests, build, download and progress lines, exact repeats and warnings
past 30 of a kind, stack frames but the first in the project (Python: the last two), the middle of
long failures. A run shorter than its marker is shown instead; three or more identical lines become
one with `[×N]`; a view over `bulky_tokens` is cut in the middle (the end holds the summaries).
Applied only when the output is over 300 tokens, the command neither shows nor searches files nor
pipes into a content filter, and the view (with its handle line and note) is at most 70% of the
output; otherwise the output takes the path above as before (unknown bulky output: the outline, and
a local summary when a local model is configured).

Metadata: sensitive paths are listed by name (and named in the task note); a search reports the
location of a match in a sensitive file but not the line; `diff` is the working-tree diff, sanitized,
with protected files' sections replaced by a note. Git commands are ordinary commands: their
output follows the command-output rules above.

### 5.3 Local roles

The local model is called by the boundary and has no tools, with one exception: the local explorer
(§5.13), which drives it through a tool loop of its own over a `LocalAgent` (a local-role endpoint
only) and whose report the boundary cleans as local output.

| Role | Trigger | Required fields |
|---|---|---|
| Brief | run start, sensitive files other than secret-bearing ones (≤20K chars each, ≤3 calls) | `summary`, `facts[]` |
| Digest | new HandleSummary, or BulkyHandle of command output (not a condensed one) | `summary` (≤800 chars per chunk), `facts[]` |
| Answer | `ask_local(handle, questions[≤6])`, one call per question on the most relevant chunk; `handle` may be a placeholder from an operator message (the handle is that message) | `answer` (≤1,200 chars), `evidence_lines[]`, `unanswerable` |
| Implement | `edit_protected(path, spec, tests?, command?)` | `code`: the whole new protected file, written by the host and validated by host-run checks |
| Condense | context compaction (§6): the older part of a conversation as the frontier was sent it, read part by part (≤60K characters each), each call updating the notes of the parts before it | `summary` (≤12,000 chars; own schema) |
| Explore | `explore {question, paths?, depth?}` (§5.13): a native tool-calling loop over the workspace as it is, read-only | a `report` call: `answer`, `findings[{path, line, end_line?, note}]` |

All roles send one shared JSON schema (servers such as oMLX key their prompt cache by schema) and
put the content before the instruction, so questions about one handle reuse the processed prefix;
each role checks its own required fields. The personal-data pass and Condense have schemas of
their own (neither shares a prefix with the others). Temperature 0; visible thinking off; content over ~60K
characters is chunked; one retry on unparseable output, then an error the frontier sees as
"unavailable" — never an invented answer.

Every local output is cleaned before it enters a view (`Engine::clean_local_counted`):
1. the 4-token copy window on the text as written (before placeholders split a copied line);
2. other spellings (`reencoded.rs`): spaced-out runs (`V a k d r i l`, `4-5-3-9`) matched against
   the vault's skeletons (letters and digits, lower-cased) and replaced by the value's token;
   base64/hex runs decoded at every alignment and, if the bytes hold a vault value (any letter
   case) or a 4-token copied window, replaced by `⟨redacted:encoded-sensitive-text⟩`;
3. sanitized as sensitive text (never across a known placeholder), then both copy windows;
4. short pieces of identifying values limited (`probing.rs`, state in `probes.json`): pieces
   tied to a position (`the first digit is`, `starts with`) are `⟨withheld:characters-of-a-value⟩`;
   in an `ask_local` answer, other 1–3 character pieces are charged to the values on the lines the
   question names or the answer cites, at most 2 characters per value over the run.

A question for characters of a value by position or piece (first/last N, the n-th, a range, a
prefix or suffix, what it starts or ends with, whether it contains some digits, spelled out,
reversed or encoded) is put to the local model as a question about the value's format, and all
of its answer's pieces of a value are withheld. Each such probe is a `local_probe` audit event
(handle, rule, pieces withheld, a running count per handle), drained from the presenter
(`Presenter::take_events`) into the run's audit log.

A compaction summary (Condense) is cleaned differently (`Engine::clean_condensed`): its input is
conversation the frontier was already sent, so it can only carry a value the model writes anyway
(recalled, echoed from an injection, invented). Steps 1 and 2 as above, then detected and vault
values replaced (as public text), copied spans, protected code and digits of withheld numbers
removed. The name and number heuristics for text about sensitive content (step 3) are not
applied: they would vault the frontier's own identifiers and replace them in every later request.

### 5.4 Outbound gate

The gate applies filters to the request, then checks, then appends to the audit log and sends.

- **Filters:** every item is sanitized again (idempotent), including the frontier's own text,
  reasoning and tool-call arguments; copied-span filter; protected-code redaction.
- **Copied-span index:** hashes of 8-token windows of every sensitive text; ≥3 consecutive
  matching windows (~24 tokens) not also present in public files are replaced.
- **Final check:** no vault value (≥6 characters, raw or JSON-escaped) may appear anywhere in the
  serialized body. A hit blocks the send (`BlockedSend` event) and the run fails; nothing is sent.
- **Audit record:** `{seq, prev, unix_ms, endpoint, model, request_sha256, request (after
  substitution), interventions[]}`; `declass audit verify` recomputes the chain.
- **Audit events** share the chain: run start (boundary on/off) and end, local-endpoint trust,
  sandbox denials, `sensitive_data` commands (command, exit code, files marked derived), blocked
  sends (check name), requests sent with parts withheld (check name, parts), protected edits, images (origin, size, digest, destination, rule),
  `ask_local` probes of a value (handle, rule, pieces withheld, count). Names, paths and outcomes
  only, never content.
- **Images in the body:** checks and the audit record see each image's data replaced by a digest
  marker (`[image sha256:…, N bytes]`; `declass_provider::image::redact`), and `request_sha256` is of
  that form; the provider sends the real body. Every check also gets the digests
  (`OutboundCheck::check_images`); the engine's refuses any image not routed to the frontier.
- **Anchor:** after every append the chain head (record count, last hash) is written to
  `<owner state>/audit-anchors/runs/<run-id>/<first-record hash>.json`, outside the workspace.
  The key belongs to the run, not to the workspace path, so a moved or re-mounted workspace still
  verifies; anchors in the earlier layout (`audit-anchors/<workspace hash>/<run-id>.json`) are still
  read. `declass audit verify` reports a log rewritten or truncated since, and a run refuses to
  continue such a log.

Canaries are not known to Declass (they carry no marker); the evaluation's leak proxy finds them.

### 5.5 Write-back

Write tools receive frontier text containing placeholders. A placeholder resolves locally only in
a secret sink (`sensitivity.secret_sinks`) or a sensitive file. Anywhere else, and for any unknown
placeholder, the write is refused with an error telling the frontier to read the value at runtime
(for example from an environment variable of that name). `edit_file` anchors may contain
placeholders; they are matched against the real text.

### 5.6 Third-party channels (web)

Everything sent to anyone other than the frontier provider and the local model is checked by the
boundary in every part, and the check is a type, as the gate is for the frontier
(`crates/declass-boundary/src/third_party.rs`, `engine/third_party.rs`; SECURITY.md, Egress):

```
tool ── Outgoing {method, url, headers, body} ──► Guard::check ──► Checked ──► declass_net::Client::send
          (declass-web, declass-mcp, the proxy)          (run's presenter:            (the only sender;
                                                    Presenter::outbound_guard)   credentials added here)
declass_net::lookup(resolver, &Checked)  ◄── the only name resolution for web tools: a checked host only
```

`Guard` holds the presenter's policy (the engine, weakly: a guard that outlives its run refuses
everything); `Checked` has no public constructor, and `declass-net` builds no request itself. The
engine's policy (`Engine::third_party_refusal`) reads the parts of a request (`Texts::of_request`:
host and labels, port, path and segments, query keys and values, headers, JSON strings and keys,
JSON inside strings; and the parts joined, and the values alone joined) for placeholders and
their starts, withheld values in every spelling it reads (as written, URL-, HTML- and
backslash-decoded, any case, reversed, spelled out, by skeleton across separators and parts,
base64/hex/base32 whole or in part: `reencoded::outbound_forms`, `base32_decodings`,
`spelled_digits`, `Vault::find_lowered`/`find_skeleton`/`find_window`), numbers made of a
withheld number's digits, and copied spans. `Presenter::check_outbound(destination, text)` is the
same check on one text. A refusal is recorded by the guard (`outbound_refused`, destination named
by `Guard::name`, never a value) and is a tool error; pass-through's guard passes everything.
`tools/gate.sh` ("egress by construction") keeps HTTP clients, sockets and resolver calls out of
every product crate but `declass-net`, `declass-provider` and the proxy's upstream connection.

The web tools (`crates/declass-agent/src/web.rs` over `declass-web`): `web_fetch {url, start_line?,
end_line?}` always, `web_search {query, count?, sources?}` only with a usable backend (`sources`
only for the native backend, an enum of the run's sources), both fixed at run start
(`RunConfig.web`; `None` when `web.enabled` is off). Flow: the presenter's guard → `declass-web`
fetches (a search engine's result page refused; each hop checked by the guard, then resolved,
every address checked, pinned; at most 5 redirects; capped; converted) →
`Presenter::present(Source::Web { url }, text)` (public-untrusted: scanned and tokenized like public
content, offloaded to a handle when bulky) → framed between markers with a per-call random tag
saying the content is data → a `web_request` audit event (tool, host, bytes, outcome).

The search backend is chosen once per run in `crates/declass-cli/src/web.rs` (`choose`) from
`web.search.backend` (`auto` by default), `web.search.sources`, the workspace root (its manifest
files pick the package registries), the run's frontier (its URL from the run manifest; none in
top-clearance runs, which offer no web tools at all) and the environment. `auto` is Z.ai's search when the frontier is Z.ai and its key is set
(`web.search.zai_engine`), else the native backend; SearXNG, Brave and Wikipedia alone only when
named. The same function feeds `declass doctor`'s "web search" check, and the tool's description
says what the backend searches (for native: each source, which are asked by default and which on
request).

A native search (`crates/declass-web/src/search/native.rs`): the agent resolves the sources to ask
(the frontier's `sources` or the defaults), checks the query once against all their hosts with the
guard, then `Web::search_with` asks each source in parallel under its own deadline: the source's
request built and checked by the guard → answer kept for the run? →
the service's pacer (one request at a time per service, its interval, GitHub's per-minute budget,
any backoff a source asked for; a turn too far off skips the source) → the guarded client (address
checked and pinned, no redirects, capped, gzip decoded) → the source's parser. Lists merge by rank
(every source's first result, then every second; a page seen once), and each source's report
(host, bytes, outcome, note) becomes one `web_request` audit event and a line of the rendered
reply, which is presented as `Source::Web` like any result.

### 5.6b Commands' network (egress proxy)

`sandbox.network` (`RunConfig.network`, `crates/declass-agent/src/egress.rs`): `off`, `registries`
(the default) or `all`. `tools::sandboxed` picks each command's `declass_sandbox::Network`: a
`sensitive_data` command gets `Off`, and so does a check when the presenter lets checks read more
than ordinary commands (protected source); ordinary commands, sub-agents' read-only commands and
the other checks get the run's mode. With `registries`, the run's `declass_egress::Proxy` is created
on the first such command (its events go to the run's audit log as `egress`), and each command gets
its own `Route`, dropped when the command ends:

```
command ── HTTPS_PROXY=http://127.0.0.1:<port> ──► route ──► Proxy::serve
  Seatbelt:   TCP to the proxy's host loopback port (and free development ports), nothing else
  bubblewrap: own network namespace; `declass __sandbox-bridge` listens on its 127.0.0.1, and hands
              each connection to the host over a channel (the helper's stdin, SCM_RIGHTS)
Proxy::serve: head (CONNECT host:port | GET/HEAD http://...) → the run's guard (the tunnel's
  host; every part of the rewritten plain request) → Hosts::allows → resolve (only a listed
  name) → every address checked (declass_web::guard) → connect → CONNECT: 200, ClientHello's server
  name == host, then bytes both ways | GET: one rewritten request, the response → Event (a host
  the guard refuses is named by `Guard::name`)
```

Before any of that, `tools::sandboxed` checks the command line of an ordinary or read-only
command that would have network (`registries` or `all`) with the guard; one holding a placeholder
or a withheld value runs with network off and a note in its output.

The agent also denies every command `declass_sandbox::home_secrets` (credential stores and shell
histories under `$HOME`), and gives commands with network package caches in the run's scratch
directory (`egress::cache_env`; `CARGO_HOME` seeded with links to the operator's crates). A refused
request is noted at the end of the command's output (`[sandbox] the egress proxy refused: ...`).
`run_command`'s description names the run's mode (the tool set stays fixed for the run).

### 5.7 Git history and commits

The git tools (`crates/declass-agent/src/git_tools.rs`; `GitTools`, decided once per run or session in
`run::tool_specs`, `None` outside a git repository) call git only through the `declass-git` runner.
History is presented per path: `git_show` lists a commit's files (`--no-renames`, first parent for
merges), drops hidden and sealed paths (`Presenter::path_visible`, `Presenter::protection`), and
presents each file's diff on its own as `Source::GitHistory { rev, path: Some(p) }`; `git_show
{path}` and `git_blame` present one path the same way; log lines, commit messages and status are
`GitHistory { path: None }`. The engine (`engine/history.rs`) classifies by the path now: sealed →
a notice; interface-only → withheld notice; sensitive → handle and summary (secret-bearing:
tokenized), with the old values registered in the vault from the file's own lines only (git's
headers, hunk ranges and blame's commit lines are split off, so hashes and modes never become
placeholders); public → scanned (`clean_public`), bulky → handle. Pass-through shows it as it is.

`git_commit` flow: message checks (no `⟨…⟩`; `present(GitHistory)` must return it unchanged, so
no vault value, detected secret or PII, or copied sensitive span) → candidate paths from the write
journal (`journal::written(run_dir)`, so across resumes and session turns) → refusals (hidden,
protected, sensitive or derived; `Git::commit_blockers`: ignored, `filter` attribute, not a regular
file) → identity (`git.author`, else repository `user.name`/`user.email`, else the owner's
`~/.gitconfig` / XDG git config read with `--file` for those two keys) → the run's base commit is
recorded in `runs/<id>/git-base` before its first commit → `Git::commit_paths`: `hash-object -w
--no-filters` per path into a private index read from HEAD, `write-tree`, `commit-tree` (author =
committer = operator, message on stdin), `update-ref HEAD <new> <old>` (compare-and-swap), then the
real index entries of those paths only → `git_commit` audit event. Approval is in
`oversight::review` (`Risk::GitCommit`: `git.commit = "ask"`, or `oversight.approve = "all"`).
`git_commit` is offered with `ask` only when the run has an approver: `oversight.approve` on, or
an interactive session (`approve::session_oversight`: a terminal on standard input gives the
session its inline approver even with approval off, and then only commits are asked).
`diff` compares with `git-base` when present, so a run's own commits never hide its changes; it
names changed sensitive files and never diffs them.

**Without a repository** (`Git::is_repository` false): `Git::list_files` falls back to
`declass_git::walk::list` (the `ignore` crate: `.gitignore` and `.ignore` files without requiring
git, no global excludes, no links followed, `.git`, `.declass` and dependency/build-output
directories skipped), so `list_files`, `search`, `code_nav`, engine priming and the TUI's IP tree
work unchanged. `diff` and `/diff` use `declass_agent::changes`: the write journal's files, each
read through the guarded file access, copied privately into the run directory and compared with
its saved first content by `git diff --no-index` (which needs no repository), headers relabelled
to workspace paths. No git tools are offered; a `git` command that fails gets a hint instead.

### 5.8 MCP servers

`crates/declass-agent/src/mcp.rs` (`Hub`) over `declass-mcp`. The CLI reads `[mcp.servers.<name>]`
(template settings `mcp.servers.*.<field>` in the registry, owner-only) and enabled native-package
server declarations into `ServerConfig`s. Package fields pass organization-policy checks before
merging; a server-name collision is an error. The CLI
starts a `Hub` in `prepare` (runs and sessions alike; `RunConfig.mcp`, `None` without servers):
each enabled server in parallel, a stdio server through `declass_sandbox::spawn` (the command profile:
`Presenter::hidden_from_commands` as deny-read, writes to the workspace and a per-server scratch
directory, network per `network`, the base environment plus the variables named in `env`; under
bubblewrap its input is a named pipe, since bubblewrap's standard input carries the seccomp
filter), an HTTP server from the host. `initialize`, then `tools/list` once: the tools become
`mcp__<server>__<tool>` (`[A-Za-z0-9_-]`, at most 64 characters, a hash suffix when shortened or
colliding), with descriptions and every schema string passed through `present(Source::Mcp {
trust: Public })` and capped (1,024 characters; schemas 8 KiB, then without docs, then a bare
object). `tool_specs` adds them to the fixed, sorted tool set.

A call (`work` in `run.rs` routes names the hub owns): approval via `Hub::action` and
`oversight::decide` (the server's `approve` under `oversight.approve`) → outbound: for a
`sensitive` stdio server every argument string is detokenized; for any other server the arguments
are checked as one JSON value by the presenter's guard (`Guard::check_value`; refusal: tool error
+ `outbound_refused`, drained into the run's log at once) → `tools/call` (over HTTP every message
is checked again by the transport's guard and sent through `declass-net`; a refusal there is
`McpError::NotSent`)
with the server's timeout, abandoned on interrupt → the rendered text through
`present(Source::Mcp { trust })` (public: scanned like public command output, bulky offloaded;
sensitive: handle and local summary), framed between random-tag markers as data → an `mcp_call`
audit event (server, tool, trust, outcome, whether placeholders were resolved). A closed transport
marks the server stopped (its process tree killed); later calls to it are tool errors. The hub is
shut down after the run or session (input closed or HTTP `DELETE`, then the tree killed).

### 5.9 Language servers

`crates/declass-agent/src/code_nav.rs` over `declass-lsp`. The CLI builds the run's servers from
`lsp.*` (`RunConfig.lsp`: the installed servers; `None` when `lsp.enabled` is off or none is
found), so `code_nav` and `rename` are in the tool set from the start or not at all. Nothing is
started until a tool needs it; one server per language then serves the rest of the run (or
session) and is shut down at its end.

Flow of `code_nav`: check the path (visible, not sensitive; positions not in protected source,
which is `hidden_from_commands` minus `hidden_from_checks`) → read the file and send it as the
current document (`didOpen`/`didChange`) → the request, to a server started with
`hidden_from_checks` plus `.git` and `.declass` denied (a server whose hidden set has since grown is
restarted) → each location: dropped if the path is not visible, else `path:line:col` (UTF-16
converted back to characters) next to the snippet or declaration text presented as
`Source::CodeNav { path, signature }` of the file it comes from (§5.1: nothing of a sensitive or
sealed file, declarations only of an interface-only file) → a `language_server` audit event (op,
file, outcome, shown and withheld counts) plus one per server start, crash or restart.

`rename` asks for the `WorkspaceEdit`, refuses it whole on any resource operation, file outside the
repository, hidden, sensitive, protected or non-source file, or text that no longer matches the
renamed name, then writes every file through the journal with a SHA-256 precondition (rolling back
the ones written if a later write fails), after `note_authored` and `resolve_for_write`, as
`edit_file` does. After `edit_file`, `write_file` and `rename`, a running server gets the new text
and `didSave`, and its diagnostics are appended once they settle (quiet for 250 ms with no
work-done progress open) within `lsp.diagnostics_wait_ms`.

### 5.10 Sub-agents

`crates/declass-agent/src/subagents.rs`. `RunConfig.subagents` (`None` when `subagents.enabled` is
off) adds `delegate {task, mode, paths?, budget?}` to the fixed tool set of runs and sessions. The
CLI builds it in `prepare` (`crates/declass-cli/src/subagents.rs`): the parent's model, or with
`subagents.model` a second provider at the frontier endpoint behind `OutboundGate::with_audit` (the
run's audit handle, the engine's filter and check), priced by that model.

A `delegate` call is dispatched in `run::work` (the loop is shared). The call and the read requests
right after it in the same response form a batch; a write request, or an invalid one, runs alone.
For each sub-agent:

```
plan     id aN; spend cap = min(subagents.max_usd, budget.usd, what is left of the parent's
         frontier_usd / sub-agents starting together); deadline = min(now + subagents.max_minutes
         or budget.minutes, the parent's deadline); the setting that binds is remembered for the
         BudgetStopped reason
start    transcript SubagentStart{child, call_id, mode, task, paths, journal_next}; audit
         subagent_start{child, mode, task_sha256, paths, model}
context  RunConfig: the parent's, no checks, no sub-agents (depth 1), the sub-agent model's price;
         Conversation: system = prompt::subagent_prompt(workspace, writes) (fixed per mode), tools =
         the parent's allowed for the mode (read: files, listings, search, diff, read-only git tools,
         code_nav, ask_local, read_raw, list_skills, load_skill, web, read-only MCP; write adds
         edit_file, write_file, rename) with its own run_command and finish, sorted; items =
         [root instructions + skill metadata + task + paths line + Presenter::task_notes()];
         child = Child{id, mode, scope, tools, the parent's stop request}. Skill authorization
         is shared with the parent through Arc<Skills>; scoped instruction state is per conversation.
loop     run::work(child cfg, driver, the same presenter, git, interrupt flag, host policy), boxed:
         transcript entries nested as Subagent{child, entry}; journal confined to the paths
         (WriteScope; read: none); calls outside its tools, sensitive_data and a finish without a
         report are refused before approval; run_command runs with Access::ReadOnly (sandbox
         Spec.read_only: workspace not writable); a session /stop ends it at its safe point
end      Completed / Failed / BudgetStopped{subagents.max_usd | budget.usd | the run's ...};
         written files from the journal range [journal_next, journal_end); SubagentEnd{...};
         audit subagent_end{child, mode, outcome, cost_usd, requests, files_written}
result   Completed: the report presented as Source::Subagent (rescanned like public text, never
         offloaded, cut at 20,000 characters) between random-tag markers, plus the files it wrote
         (created N lines / modified +a -r, visible paths only); otherwise a tool error saying
         why, with the files it wrote (kept)
```

The batch runs with `buffered(subagents.max_parallel)` on the parent's task, so results come back
in call order; a local-model call inside a sub-agent (`block_in_place`) pauses the others' polling
for its duration, frontier requests stay concurrent. Each sub-agent's `RunStats` is added to the
parent's (cost, usage, failed attempts, ledger) and to `RunStats.subagents` (children, requests,
tool calls, cost, usage); the parent's journal takes up numbering after a writing sub-agent
(`WriteJournal::refresh`).

**Resume.** `run::replay` rebuilds each sub-agent's spend from its nested entries (its own
`replay_priced`). Then `subagents::recover` (one-shot resume, session open and each session turn
start) ends every sub-agent whose `delegate` call has no result in the conversation the parent
continues with (`Failed`, "the run stopped before its result was recorded", with an audit event)
and, for a writing one, restores the files of its journal range to their content before it,
except files a later journaled write changed (`SubagentReverted{child, paths}`); the parent then
re-decides the dropped step. A sub-agent whose result was recorded keeps its writes, and a session
`/undo` of its turn reverts them with the parent's.

### 5.11 Images

`crates/declass-agent/src/images.rs` over `declass-provider`'s `image` and `declass-boundary`'s `images`.
Entry points: `read_file` on a path with an image extension (routed by `run::work` before the
tool dispatcher), `RunConfig.images.attached` (`declass run --image` / `--image-public`, attached to
the task), and `Session::attach` (`/image` in a session, attached to the next
message). `images::precheck` applies the same rule from the policy alone before a run or session
exists (the CLI before creating a run), so a refusal shows at once.

```
read / attach ─ prepare (sniff PNG/JPEG/GIF/WebP, decode with size and allocation limits,
                scale to images.max_side, re-encode: JPEG stays JPEG, the rest PNG; ≤3.75 MB)
              ─ Presenter::route_image(origin, operator_public, sha256, frontier.vision)
                  pass-through: Frontier if frontier.vision, else Refuse
                  engine: protected path → Refuse; sensitive path (policy or derived) →
                    Describe (local.vision) or Refuse, never Frontier; public (operator mark, or
                    images.to_frontier = public and a workspace path) → Frontier if
                    frontier.vision, else Describe/Refuse; otherwise Describe or Refuse.
                    Frontier records the digest (frontier-images.json).
              ─ audit `image` {origin, bytes, sha256, destination, decision, operator_public}
              ─ Frontier: store images/<sha256>.<ext>; tool result or message note names it;
                  Item::Images { call_id } follows the ToolResult / User item
                Describe: present(Source::Image { origin }, bytes) → handle + local
                  description (image before the instruction), cleaned as sensitive text with
                  every name-like phrase a person, copied spans removed; ask_local on the
                  handle shows the local model the image again
                Refuse: tool error, or (attachment) the run fails / the turn fails before sending
```

Dialects: Chat Completions puts images before the text of a user message (`image_url` data URL)
and sends tool-result images as one user message after the run of tool messages (tool messages
are text only); Anthropic puts `image` blocks (base64 source) before the text, and inside the
`tool_result` content; Responses uses `input_image` parts in the user message and in
`function_call_output.output`. An `Images` item whose images are not loaded or were masked sends
nothing. The gate's outbound filter drops images whose digest was not routed to the frontier, and
the check refuses any that remains. Resume and session resume load each image back from the store
by digest (one that is missing or altered is dropped). The engine does not index image bytes as
text when primed. A sub-agent runs the same `work` loop, so its `read_file` on an image takes the
same path into its own conversation; its configuration keeps `frontier.vision` only when the
parent's model drives it and never carries the operator's attachments.

### 5.12 Structure views, synthetic samples, masked output

`crates/declass-boundary/src/structure/` (pure: parsing, profiles, shapes, dates, twins, masking,
over a `Knowledge` trait) and `engine/structure.rs` (the engine's `Knowledge`: vault spans, the
public and schema words, the values of structured sensitive data; state, checks, audit).

```
prime / mark_sensitive ─ index_structure(path, text)
    string values of JSON/CSV/TSV/JSONL/XML/fixed-width → data values (4+ chars, Aho-Corasick,
    whole words, case ignored) and one-word values → data words
    policy-sensitive, not rewritten by a command → outline (format, layout, fields and types) → task note
read_file (numbered) → present(File) → handle_view_as(File)
    strip line numbers → probe_gate (a short file a sensitive command wrote) → handle
    → error lines → profile(text, path): detect format → parse with spans → per-field stats
      → render (fields, shapes, pictures, anomalies; a log: layout, levels, line templates)
    → inline_sample (policy-sensitive data file): twin(rows = 1) → checks → first record
run_command(sensitive_data) → present(SensitiveCommand) → handle_view_as(Command)
    probe_gate: ≤200 program characters → output_probe {count, budget, shown}; past
      sensitivity.output_probes → fixed text (no handle, size, exit code, local call)
    → masked_section (≤80 lines, ≤6,000 chars): sanitize (vault learns its values) →
      mask: value spans (vault, data values) as shapes (secrets •••), dates by layout, words kept
      only if public/schema/command words and not data words, numbers per the aggregate rule
      (0-99 while sensitivity.masked_numbers lasts) → masked_numbers event
synthetic_sample {handle, rows} → policy-sensitive file only → twin(text, path, seed, rows)
    per value: fake = f(shape, kind, first position, seed); redrawn while it holds a known value
    records: the first, then greedy cover of (path, shape class, null, missing) features
    check: no vault value (JSON literals aside), no copied span → else withheld (3 seeds)
    shown: its lines → fixture set, its header → public text, its detected fakes → authored
resolve_for_write(path, content) → note_fixture: every line from a sample shown and path under a
    sensitivity glob → fixtures.json {path: sha256}; is_sensitive(path) is false while the file's
    bytes hash to that digest
```

Formats: JSON (records: the array itself, or the longest array of objects under a top-level key),
JSON lines, delimited text (delimiter sniffed, RFC 4180 quoting, header detected), `KEY=value`,
fixed-width (columns from blank gaps), simple XML (records: the root's children), logs (timestamp
or level at line start) and other text (templates only). Dates are recognized by layout
(`dates.rs`: ISO, compact `yyyyMMddHHmmss`, day/month orders, month names, zones) and generated in
the same layout. Fakes: cards Luhn-valid with a drawn network prefix for their length; IBANs for a
registry country of the same length, mod-97 valid; ids and phone numbers searched until the
detectors recognize them; emails at `.test`; numbers with the same digits, decimals, sign, epoch
plausibility and side of 2^53; letters pronounceable with the same case and UTF-8 length.
The seed is drawn per run (`sample-seed`); equal values get equal fakes; a larger sample starts
with the smaller one's records.

### 5.13 Local explorer

`crates/declass-agent/src/explore.rs`. `RunConfig.explore` (`None` unless `explore.enabled` and a
local model is enabled; never in a sub-agent) adds `explore {question, paths?, depth?}` to the fixed
tool set of runs and sessions, in hybrid and pass-through; the run and session prompts name it only
then (so the prefix of a run without it is the one every evaluation measured). The CLI builds it in
`prepare` (`crates/declass-cli/src/explore.rs`): the configured local model through `local_provider`
(the same endpoint rules and deadline as the engine's), wrapped in `LocalAgent`, which refuses a
provider that is not in the local role and implements the `Driver` trait.

```
call     parse: question (≤4,000 chars), paths (≤20 globs, relative, no .git/.declass), depth quick
         (default) or thorough → Caps{steps, time, bytes} (explore.quick_steps / thorough_steps;
         explore.max_seconds, a third for quick; explore.max_read_kb, half for quick); until =
         min(now + time, the run's deadline)
view     SecureView over the calling loop's presenter: present() shows content as it is, cut at
         12 KB per result, listings and searches kept to `paths`, and records every text with the
         file it came from (search lines split per file); path_visible adds .git/.declass to the
         presenter's hidden paths; path_sensitive/protection/hidden_* delegate; resolve_for_write
         refuses
tools    the calling loop's implementations (tools::dispatch) with a Ctx over the view: read_file
         (text), list_files, search, code_nav (if the run has language servers), git_log/show/
         blame/status (commits off), report; sorted; any other name refused before dispatch; no
         network, no web, no MCP, no sub-agents
loop     system = explorer prompt (workspace name only); items = [question (placeholders resolved;
         a positional question put as one about format), where to look, the budget]; each step
         one LocalAgent request (temperature 0.2, visible thinking off, ≤2,048 output tokens) under
         timeout_at(until) and the interrupt flag; tool results counted against the byte cap and cut
         to what is left; when steps or bytes run out one last request offers only `report`; plain
         text instead of a call is taken as the answer
report   findings kept only for a visible, non-reserved file the explorer was shown, with the line
         inside it (≤25); the answer and notes (note lines marked) presented together as
         Source::Explore{question, read} → engine: index what was read (sensitive as sensitive,
         protected through ip_index, public as public), withhold values of structured sensitive
         data (with structure views; kept only when all words are public or schema words, never
         with a digit), clean_local_counted over all of it as an answer to the question, then
         ip_redact; paths presented as Source::FileList (protected
         ones marked); the line at a reference quoted by Declass only for an open file, as
         Source::CodeNav; framed between random-tag markers
end      Explored{call_id, stats} in the transcript; audit `explore` {question_sha256, depth,
         steps, files, bytes_read, local_seconds, references, outcome}; the presenter's probe
         events drained; ledger.explore (calls, reported, steps, bytes, local CallStats); the
         result is shown as LocalAnswer
```

Outcomes: `reported`; `partial` (a cap ended it before a report: the frontier gets the files it
read and nothing else); `failed` (the local model errored: a tool error); `interrupted`. The
explorer's own conversation is not kept (it held raw content). **Resume**: `Explored` entries
rebuild `ledger.explore` in `run::replay`; a call whose result was not recorded is dropped with its
turn and decided again, like any tool call (nothing to roll back: it never writes).

### 5.14 Instructions, skills and native packages

`declass-agent::instructions` loads standing owner files and repository-root conventions in a
deterministic order. `Conversation.scoped_instructions` tracks descendant instruction identities
and digests by target directory. It walks outward-to-inward scopes, skips root files already in
the opening message, caps each rendered block at 32 KiB, and supplies changed rules before a
named file operation. Replay and context discard clear this cache. The guard covers the initial
path of a rename; shell paths and other affected files are not enumerated. The system prompt
asks the model to read files before shell edits. These are guidance rules below host/operator
authority, not changes to file permissions or execution policy.

`declass-cli::extensions` discovers owner/project skill roots and enabled package contributions.
The `declass-extensions::Catalog` reads bounded `SKILL.md` files to retain metadata and digests;
full bodies stay out of startup model context. `RunConfig.skills` holds `Arc<skills::Skills>`,
which wraps the catalog and explicit host authorizations. `/skill` validates `user-invocable`
and authorizes the exact ID; a model cannot grant that authorization. `list_skills` pages a
filtered catalog, and `load_skill` enforces `disable-model-invocation`, path visibility, document
caps and the current instruction digest. Project content uses `Source::File`; owner/plugin
content uses `sanitize_message`. Each loaded document produces an `instructions` audit event
with `skill:<id>` and its digest. Authorization is recreated after resume; recorded content is
replayed normally. Skill metadata such as `allowed-tools` never modifies the tool set.

`declass-cli::plugins` owns native package inspection, installation, enabled records and command/MCP
adaptation. Installation reads a local bounded tree through pinned no-follow handles, validates
`declass-plugin.toml` schema 1, and writes a private content-addressed snapshot and record. It runs
no install scripts or dependency manager. Limits include 512 files, 8 MiB per file and 32 MiB
per package; paths cannot escape the package. Activation checks the content SHA-256 and skips
disabled snapshots. Plugin skills use `<plugin>:<skill>` IDs. `/command plugin:name` rechecks
package content, reads bounded Markdown and appends operator arguments as a sanitized user
message; it does no shell or template substitution.

Enabled package MCP declarations pass `Config::allows` for every contributed setting before
joining the regular MCP startup path (§5.8). They inherit sandbox, data filtering, approvals and
top-clearance exclusions. Default package trust is `sensitive`, stdio network is off and approval
is `always` under the configured oversight mode. Package hashes detect content changes, not
publisher identity; installed programs remain part of the owner's trust decision. Foreign plugin
ABIs/hooks, marketplaces, signatures and automatic dependency resolution are not implemented.
See [extension formats and supported compatibility](docs/EXTENSIONS.md) and
[extension threats and limits](SECURITY.md#portable-skills-and-native-plugins).

## 6. Context management

- Transcript is append-only and is the source of every request, so the provider prefix stays
  stable across turns.
- Masking decisions use a size estimate of the request; costs use the provider's reported usage
  (estimated when a server reports none, and counted separately when unknown).
- Above `context.mask_at` (0.7) of `context.window_tokens` (200K), tool results are replaced at once, whole turns at a time
  (oldest first, the last 4 turns kept, results under 400 characters kept), by stubs naming the
  call and any handle: `[masked: run_command `cargo test` → h7, ~3100 tokens removed to save
  context; h7 is still available (...)]`. Tool calls and their results are never separated.
  Masking happens rarely and in batches so caches are invalidated rarely. Images count by their
  own estimate (about one token per 750 pixels); a masked turn's images are dropped with their
  result, whose stub then says to repeat the call.
- **Compaction** (`context.compaction`, off by default until measured; `declass_agent::compaction`).
  At each safe point, when the estimate passes `context.compact_at` (100K tokens), one event
  brings the conversation down to `context.compact_to` (0.4) of it:

  ```
  estimate ≥ compact_at (and ≥ retry_at after a failure)
    1. masking first: the stubs above, toward the target; if that reaches it, done (no local call)
    2. else compaction: first message (and its images) | older part | recent turns
         recent = as many whole turns as fit in the target with a summary, ≥ the last 4;
                  the cut is where a turn starts, so a call keeps its result
         older part: Driver::as_sent (the gate's filters) → rendered, results and arguments
                  shortened (files can be read again) → Presenter::condense
                  → LocalReader::condense (§5.3) → cleaned as local output (engine)
         one user message replaces it: "[compacted: N earlier items (~T tokens) ...]" + notes
                  (+ in a session, the operator's latest message verbatim if it was condensed)
    3. then the window's own masking, as always (the safety net)
  ```

  An event needs an older part of at least a quarter of `compact_at`, so events are rare and
  each breaks the provider's prefix cache once; the system prompt, tools and first message keep
  theirs, and requests after an event only append. No summary (no local model, an error, an empty
  or too long one, one no shorter than what it replaces, an older part over 240K characters)
  changes nothing: `compaction_failed` is recorded, masking goes on, and no attempt is made until
  the conversation has grown by a quarter of `compact_at`. Compaction never ends a run.
  Pass-through, and hybrid with `local.enabled` off, have no local model, so they never compact
  (no attempt, no failure recorded). Each conversation (a session, a
  sub-agent) compacts on its own; sub-agents inherit the settings.
- **Resume.** Each event is recorded before it changes the conversation, and one that could not
  be recorded is not applied: `masked` holds its positions, `compacted` holds `head`, `upto` and
  the replacement text (and tokens before/after, local seconds), `compaction_failed` the estimate
  at which to retry. Replay applies them where they happened, so a resumed run sends the same
  request byte for byte (masking recorded before positions were is re-decided as before). A
  sub-agent result condensed away still counts as recorded when a resume looks for unfinished
  sub-agents.
- The run's cost ledger (`summary.json` `stats.ledger`) charges every tool result, for each
  request that carries it, to the class it was shown as (raw, tokenized, handle summary, local
  answer, bulky handle), and counts `ask_local` calls and questions, `sensitive_data` commands,
  sandbox denials and local busy seconds; images by destination (sent, described, refused) with
  their estimated share of the reported input dollars (`stats.ledger.images`); the method is
  documented in `declass-agent/src/ledger.rs`.

## 7. State on disk

All Declass state lives under the workspace's `.declass/`, classified by the path registry (ignored by
git; reset behaviour defined per entry).

```
.declass/
  config.toml                 project settings (tighten-only)
  git/                        private checkpoint store (bare, fixed config, flock)
  runs/<run-id>/              mode 0700, files 0600; `declass purge` after data.retention_days
    run.json                  manifest (mode, task, frontier; `session` for a session) for resume
    transcript.jsonl          full conversation items, synced per item; for a session also its
                              turns (as typed), their ends, steering and undo; sub-agents'
                              starts, ends, rollbacks and their own entries nested under their id;
                              masking positions and compactions (with their replacement text);
                              the explorer's calls (counts only, never what it read)
    handles/<hN>(.source)     raw bytes of handles (local only)
    vault.json                placeholder ↔ value map, aliases (local only)
    derived.json              files made sensitive by `sensitive_data` commands
    probes.json               characters of each value local answers showed; probes per handle;
                              short sensitive outputs and small masked numbers shown
    sample-seed               seed of the run's synthetic samples
    fixtures.json             files written from sample lines only → digest of that content
    rewritten.json            files `sensitive_data` commands wrote (no outline or sample of them)
    operator.json             placeholders of operator-typed values → the handle of their message
    spill-<uuid>.txt          long command outputs
    diff-<uuid>.tmp           private copy of a file while `diff` compares it outside git (removed)
    writes.jsonl              pending/applied records for crash recovery
    git-base                  HEAD before the run's first git_commit (what `diff` compares with)
    images/<sha256>.<ext>     images the frontier was shown, by digest (the transcript holds digests)
    frontier-images.json      digests of the images routed to the frontier (the gate sends no other)
    summary.json              terminal state (placeholders restored for the operator), usage, cost ledger
  audit/<run-id>.jsonl        hash-chained outbound log (placeholder-substituted)
  lock, tmp/                  workspace lock; sandbox scratch space
~/.config/declass/config.toml    owner settings (credentials, endpoints, local address, policy)
~/.config/declass/DECLASS.md        the owner's standing instructions (optional; next to config.toml)
~/.config/declass/AGENTS.md      optional owner rules; AGENTS.override.md replaces this file
~/.config/declass/CLAUDE.md      optional owner rules (GEMINI.md also supported)
~/.config/declass/skills/        owner <name>/SKILL.md workflows (plus compatible discovery roots)
~/.config/declass/plugins/      private native package store (beside the configured owner file)
  <name>.json                installed digest and enabled state
  <name>/<sha256>/           package snapshot; retained after unregistering
~/.local/state/declass/          owner state ($DECLASS_CONFIG_HOME/state when set), mode 0700
  audit-anchors/runs/<run>/<first>.json   chain head of each run's audit log
  config-audit.jsonl          hash-chained log of `declass config set` changes
```

## 8. Safety primitives

- **File I/O:** all workspace access goes through `PinnedParent` (`O_NOFOLLOW` walk, `openat`,
  `renameat`, `fsync`); writes are atomic with digest preconditions and rollback; `.git` and
  `.declass` are never writable by tools.
- **Commands:** Seatbelt (macOS) or bwrap (Linux) with absolute binary paths; workspace-write with
  `.git`/`.declass` unwritable; `.declass` (the vault, handles, transcripts, audit log) unreadable to
  every command by the sandbox itself, `.git` (committed copies) to ordinary commands and checks;
  command `TMPDIR` outside the workspace (a sub-agent's commands can write nothing else: the
  workspace is read-only to them, `Spec.read_only`), a separate one for `sensitive_data` commands that no
  other sandboxed process (command, check, MCP server) can read (`tools::hidden_from_processes`), where
  their cargo builds go too (`CARGO_TARGET_DIR`); sensitive and protected paths unreadable (deny-read) except for
  `sensitive_data` commands (whose placeholders are resolved locally), and protected source readable by the host's checks; credential
  stores in the home directory unreadable to every command; network only through the egress proxy
  by default (§5.6b), none for `sensitive_data` commands and checks that read protected source;
  tmpfs `/run`; restricted service lookup; process-tree kill on timeout or interrupt.
  On Linux: denied paths covered by mode-000 stand-ins, no capabilities, a seccomp filter against
  Unix sockets unless the network is unrestricted, and `.git`/`.declass` entries a command created removed when
  it ends. A sandbox that cannot start refuses the command.
- **Git:** one helper; environment cleared; fsmonitor, hooks, filters and user/system config
  disabled.
- **Credentials:** the owner config names an environment variable (`frontier.api_key_env`,
  `local.api_key_env`); the value is read at request time and never written to config, transcripts
  or audit.

## 9. Configuration

`declass-config` holds a registry of every setting:

```rust
struct Setting { key, kind: Bool | Int{min,max} | Float{min,max} | Str | List | Choice,
                 default, scope: Owner | Project,
                 direction: Any | AddOnly | RemoveOnly | OnlyTrue | OnlyFalse | OnlyLower | OnlyLaterChoice,
                 confirm: bool, help }
```

Loading merges defaults → owner file → project file, rejecting owner-only keys in project files
and any project value that loosens privacy; reading an unregistered key is an error. A program that
embeds Declass may add a policy layer on top (`Config::load_with`, §13); Declass's own command line
loads none. `frontier.allow_passthrough` (on by default; a project may turn it off) decides whether
`--mode passthrough` may run in a repository, for new and resumed runs and sessions alike. A value in a
form an earlier version wrote is read as its current meaning with a note (`migrate`:
`sandbox.network = true` / `false` read as `"all"` / `"off"`). `declass config
list|get|set` and the workspace's settings screens are driven by the registry and share `Config::propose` / `apply`. `declass config set` refuses a
change that loosens privacy (a `confirm` setting, or a value against its direction) without
`--confirm`, prints the diff and appends every applied change to the owner's `config-audit.jsonl`.

## 10. Termination and recovery

Every run ends in exactly one `Terminal` state, with `summary.json` and the audit log's `run_end`
event (which anchors the log's final head) written by `declass_agent::conclude_with` (which then
calls an embedding program's end hooks, §13):

- `Completed{summary}`: `finish` passed the checks.
- `Failed{reason}`: the task cannot be completed as things stand: an error a retry cannot fix
  (credentials, an invalid request, a context overflow), a send the outbound gate still blocks
  after withholding the parts that held a value,
  `content_filter`, repeated `length` stops or text-only turns, checks still failing after
  `limits.max_finish_attempts`, an interrupt (`interrupted; resume with declass resume`), or a
  panic anywhere in the run (`internal error: <message>`). `declass_agent::run` catches panics in the
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
- Writes record `pending` before and `applied` after; `declass resume <run>` reconciles pending writes,
  drops a partly answered turn and continues the transcript and the same audit chain. It refuses a
  completed run and a malformed run id.
- Ctrl-C stops a frontier wait or a retry at once, kills a running command's whole process tree
  (checks included), records the interrupted call in the transcript (its result is not recorded)
  and ends in resumable `Failed{interrupted}`.

A session applies the same states per turn (`TurnEnd`, §4 Sessions) and stays open after any of
them; each `declass` session invocation concludes the run with the session's state (`Completed` when the
operator closed it, resumable `Failed{session left open}` when they left, `BudgetStopped` when a
session budget is spent). `declass resume` refuses a session; `declass --resume` continues it.

A full disk is an infrastructure failure too (`declass_agent::host`). A write of run state that fails
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

`declass-eval` treats every agent — Declass in its three modes and external agents — as a black box
behind a **logging reverse proxy** that sits between the agent and its frontier endpoint.

```
task workspace (with canaries) ──► agent lane ──► leakproxy ──► frontier API
                                                     │
                                                leaks.jsonl  (independent of Declass's audit log)
workspace ──► sealed grader (hidden tests, secret-sink scan) ──► judge (code quality) ──► stats ──► report
```

Tasks come from the tiered dogfood suite (docs/DOGFOOD_SUITE.md). Gates use paired designs:
quality passes when the judge score is non-inferior (lower bound of the paired difference > −2/30)
and the candidate is not behind on hidden-test pass rate on a majority of tasks; cost needs the
upper bound of the paired difference < 0; privacy needs zero leaks. Declass runs write their cost
ledger to `summary.json`, which `declass-eval report` reads per lane.

The cost profile (`crates/declass-evals/src/profile.rs`) is computed from what a run already left:
per request, the context sent and the output from the usage in the proxy's response capture (every
lane alike); per turn, its cause from the tool calls of Declass's `transcript.jsonl` (sub-agents
included). `report --project-prices` re-prices each run's `usage_by_model` at other verified rows
of `pricing.toml`. Both go into the batch's `report.md` and `cost-profile.json`.

A run must not take over the machine it runs on (`crates/declass-evals/src/governor.rs`). A lane's
agent and each grading command are started at a scheduling priority every descendant inherits (a
QoS clamp through `taskpolicy -c utility` on macOS, `nice` elsewhere) and watched by the memory
governor (`crates/declass-governor`, the one that also holds Declass's own commands and servers),
because the kernel cannot bound a process tree's memory on macOS (a spawn-time limit does not
reach children; the runaway that prompted it was a test binary three levels below the lane). The
governor reads the process table in-process every 2 s to follow the tree from its root (members
whose parent died stay members; identity is id plus start time) and the members' physical
footprint (resident plus compressed or swapped memory) every 0.5 s (about 0.6% of one core in
all); it kills a member above the per-process limit, the largest member while the tree is above
the per-run limit, the largest while the machine is critically short of memory, and every member
still alive when the step ends, so a timeout ends the whole tree and no orphan holds a grading
command's output open.
Kills, peaks and CPU go into the run record. Build output is deleted after grading.

Declass's security engine takes its local reader as an option: `local.enabled = false` opens it with
none (the `declass-hybrid-nolocal` lane), and every local-backed feature then either degrades to the
detectors' view (handle summaries, bulky previews, the brief) or is refused (`ask_local`,
`edit_protected`, image descriptions, and runs with `sensitivity.local_pii_pass` on).

## 12. Invariants

Each is backed by a test, except where noted.

1. No code path reaches the network with a frontier request except through `OutboundGate`.
2. No vault value, canary or ≥24-token span of a sensitive handle appears in any audit record.
3. The local provider's endpoint is loopback or owner-allowlisted, and a remote one uses TLS unless
   the owner set `local.allow_plaintext`.
4. Project configuration cannot loosen privacy or set owner-only keys.
5. The system prompt and tool list are byte-identical for every turn of a run (as sent, they change
   only if the vault learns a value they hold, which the gate then replaces).
6. A tool call is never persisted or sent without its result.
7. Every run ends in a `Terminal` state, with a summary and an audit end event, also on a panic;
   no run exits with pending writes unrecorded.
8. `.git` and `.declass` are not writable by tools or sandboxed commands, and `.declass` is not
   readable by any sandboxed command.
9. No file under `crates/` contains another coding agent's code, format or name (outside eval lane
   adapters).
10. A sub-agent has no tool its parent lacks, cannot delegate, writes only through the journal
    within its paths (its commands cannot write the workspace), and its requests pass the same gate.
    The local explorer has only reading tools, sends raw content only to a local-role endpoint, and
    its report reaches the frontier only through `Presenter::present` (`Source::Explore`).
11. A sandboxed command reaches no network but the egress proxy (and its own loopback servers)
    unless `sandbox.network = "all"`; a `sensitive_data` command reaches none in any mode; the proxy
    resolves only listed names and connects only to public addresses.
12. No product code sends to a third party except `declass-net`, which sends only a `Checked`
    request, and only `Guard::check` (a presenter's policy over every part of the request) makes
    one; the other code that opens connections is `declass-provider` (the frontier and the local
    model) and the egress proxy's upstream (gate: "egress by construction"; the egress oracle).
13. With a policy layer loaded (§13), no file, settings change, preset, command-line override or
    recorded run value moves a setting past it, a required or direction-less key never changes,
    and a policy source that fails leaves nothing running (`declass-config` policy tests,
    `declass-cli` overrides tests).
14. Hooks of an embedding program are told no content and cannot change a run: a subscriber gets
    every record appended after it was attached, in order, without request bodies; end hooks run
    exactly once per run or session invocation in every terminal state; a failing hook becomes a
    `hook_failed` event (`declass-cli/tests/embedding.rs`, including a hybrid run with planted values).

## 13. Embedding Declass

A program may link Declass's crates and build its own binary on them; Declass Enterprise does. It calls
`declass_cli::main_with` with what it adds (point 4 below) and gets `declass`'s whole command line,
with runs and sessions composed exactly as `declass` composes them (`prepare`, `execute` and `chat`
in `crates/declass-cli/src`). Three generic extension points let such a program add to what Declass does
without changing how a run behaves. The `declass` binary is `main_with(Embedding::default())`: the
same calls with none of them (no policy source, `Hooks::default()`), so Declass's own behaviour does
not depend on them. What they see is what Declass already records: names, counts, digests, chain
positions and paths, never content.

**1. Policy layer** (`declass_config::policy`, re-exported by `declass_agent::embed`). A layer above the
owner's and the project's configuration files:

```rust
pub trait PolicySource: Send + Sync {
    /// The policy as TOML in the settings' own layout, or None when no policy applies.
    fn load(&self) -> Result<Option<(String, PolicyMeta)>, PolicyError>;
}
PolicyMeta { origin, verified_by /* set by the source */, name, version, sha256 /* set on load */ }
Config::load_with(owner: &Path, project: Option<&Path>, policy: Option<&dyn PolicySource>)
    -> Result<Config, ConfigError>
Config::policy() -> Option<&Policy>;  Config::policy_rule(key) -> Option<String>
Config::policy_overrides() -> &[String];  Config::allows(key, &Value) -> Result<(), ConfigError>
```

An optional `[policy]` table names the policy (`name`, `version`) and lists `required` keys; every
other key is a registered setting (or an instance of a template setting) with a valid value.
- A setting with a tighten direction (the registry's `Direction`) may only be as tight as the
  policy's value or tighter. Where a file is looser, the tighter of the two applies (the union of
  add-only lists, the intersection of allowlists, the lower number, the stricter choice, the tighter
  boolean), origin `policy`, with a note naming the file. A change past it is refused
  (`ConfigError::Policy`, confirmed or not) by `Config::propose`, `apply`, `set_owner_checked`,
  `set_owner` and `set_project`, so by `declass config set`, `declass config preset` and the TUI.
- A setting without a direction, and every key in `required`, is fixed at the policy's value: no
  layer may change it, not even to tighten it.
- Values that do not come from the files are checked with `Config::allows` before a run or
  session starts, and again in `prepare` (`crates/declass-cli/src/overrides.rs`): `--frontier-url`,
  `--frontier-model`, the dialect, endpoints and mode a resumed run or session recorded, and the
  local server the bootstrap found (`DECLASS_LOCAL_PORTS` only chooses where it looks). `--mode
  passthrough` (and so `--no-privacy`) is governed by `frontier.allow_passthrough`; an image marked
  `--image-public` reaches the frontier only with `frontier.vision`. `DECLASS_CONFIG_HOME` only chooses which owner file is read: the policy bounds
  whichever it is.
- Instances of template settings (`mcp.servers.<name>.*`, `lsp.servers.<language>.*`) are bounded
  when the files configure them; a policy does not create one.
- Fail closed: a source that is configured but fails (not found, not verified, not valid) makes
  loading fail with `ConfigError::PolicyLoad` ("… declass does not run without its policy"), so
  nothing runs. `Ok(None)` means no policy applies.
- It explains itself: `declass config list` shows the policy (`# policy layer: …`) and each bound key
  as origin `policy` with its rule, the TUI's detail pane shows the rule, a refused change names
  the policy and the rule, and `declass doctor` adds a `policy` check (which policy, its digest, and
  the owner or project values it overrides, as a warning).

`PolicyFile::new(path)` is the reference source: an unsigned TOML file (a missing file is an
error). Nothing in Declass's own command line configures a policy source; signed,
organisation-managed policy is a Declass Enterprise feature.

**2. Audit subscribers** (`declass_boundary::audit`). An embedding program follows a run's audit log
as it is written:

```rust
pub trait AuditSubscriber: Send + Sync {
    fn name(&self) -> &str;
    fn opened(&self, log: &Opened) -> Result<(), String> { Ok(()) }   // where the chain stands
    fn appended(&self, record: &Appended) -> Result<(), String>;
}
Opened   { log, run_id, records, head }
Appended { seq, prev, hash /* SHA-256 of the line: the new head */, unix_ms, record: Recorded }
Recorded::Event { event: AuditEvent }            // as recorded
Recorded::Request { endpoint, model, request_sha256, bytes, interventions }   // never the body
AuditLog::subscribe(Arc<dyn AuditSubscriber>);  AuditHandle::subscribe(..);  ::chain_head()
```

Delivery: every record appended after the subscriber was attached, once, in chain order, after its
line is appended and synced and its anchor written (or the anchor write failed); records already in
the log (a resumed run) are not delivered, `opened` says where the chain stood. Calls are made on
the appending thread with the log locked: a subscriber must return quickly (queue slow work such as
network export on its own thread) and must not append to the same log (that deadlocks). Several
subscribers are told in the order they were attached. Failure: an error or a panic is caught,
printed on stderr and recorded as a `hook_failed` event (`hook`, `stage` = `open` or `audit`, the
`seq` it missed, `panicked`; never its message), which every subscriber is then told of; a failure
to deliver that record is not recorded again. The run is not otherwise affected, and a subscriber
can rebuild what it missed from the log (`audit::read`).

**3. End hooks** (`declass_agent::embed`). Called once when a run or a session invocation ends:

```rust
pub trait EndHook: Send + Sync {
    fn name(&self) -> &str;
    fn ended(&self, report: &EndReport) -> Result<(), String>;
}
EndReport { run_id, kind: Run | Session, mode, resumed, state /* completed | failed | budget_stopped */,
            budget, interrupted, resumable, audit_log, chain: Option<ChainHead { log, records, head, anchor }>,
            policy: Option<PolicyMeta>, stats: EndStats /* turns, tool calls, tokens, cost, seconds */,
            disclosure: Option<Disclosure> /* counts by class, as in summary.json */ }
declass_agent::conclude_with(run_dir, run_id, audit, audit_log, &terminal, &stats, &Ending {
    kind, mode, resumed, policy, hooks })
```

`conclude_with` records `run_end` (anchoring the head), writes `summary.json`, then calls each end
hook, whether or not the summary could be written. Every `declass run`, `declass resume` and `declass` session
invocation that got as far as creating its run directory ends there exactly once, in every
terminal state: completed, failed (including a panic and a failure before the audit log opened,
then `chain` is `None`), budget-stopped, interrupted (`interrupted`, `resumable`), a session left
open (`failed`, `resumable`) or closed (`completed`). A run interrupted and then resumed is one call
per invocation (`resumed` on the second). The report holds no task, summary, failure reason or
other text a model wrote or the workspace held. A failing hook is recorded as `hook_failed` (stage
`end`) after `run_end`, so the reported head is the head at `run_end` and only such records follow
it until the run is resumed. A process killed outright (SIGKILL, power loss) calls nothing.

**4. The command line as a library** (`declass_cli`). The `declass` binary's `main` is one call:

```rust
pub fn main_with(embedding: Embedding) -> ExitCode;      // fn main() -> ExitCode { main_with(e) }
Embedding::new(Product::new("Name", "x.y.z"))            // --version: "Name x.y.z"
    .with_policy(Arc<dyn PolicySource>)                   // every command that loads settings
    .with_hooks(Hooks)                                    // every run and session invocation
    .with_doctor_check(Arc<dyn DoctorCheck>)              // `declass doctor` and the TUI's doctor
pub trait DoctorCheck: Send + Sync { fn run(&self, ctx: &DoctorContext<'_>) -> Vec<Check>; }
DoctorContext { workspace, config: Option<&Config> /* with the policy layer */, online }
Check::new(name, Status::{Skip, Pass, Warn, Fail}, detail).fix(how)
```

`main_with` reads the process's arguments, builds the asynchronous runtime (so it must not be
called inside one) and returns the exit code `declass` would end with; an error is printed as
`Error: …` and ends with 1, and `--help`, `--version` and argument errors end the process as
`declass`'s do. It is also the egress bridge helper inside a bubblewrap sandbox (the sandbox starts
the running executable with `__sandbox-bridge`), so the helper is the embedding program's own
binary. What `Embedding` adds, and where:
- the policy source: `Config::load_with` everywhere `declass` loads configuration (`run`, `resume`,
  a session, `config`, `purge`, `local-eval`, `doctor`, the workspace's settings screens and cache probe);
- the hooks: attached to a run's audit log as soon as it is opened for this invocation (before
  `run_start`, or before `run_end` of a run that failed before that) and passed to
  `conclude_with`, for `declass run`, `declass resume` and a `declass` session;
- the product: `--version` prints `<name> <version>`; `declass doctor`'s `version` line names it and
  the Declass it is built on (`…; Declass Core 0.1.0`), and its JSON adds `product` and `core_version`
  (`version` is the product's); `declass` itself prints exactly what it always did;
- the doctor checks: run after Declass's own, counted in the exit code like any other.

Nothing else changes: the same commands, flags, messages, files and exit codes. Tested with a
test binary that is such a program (`crates/declass-cli/tests/embedded.rs`).

**Composing a run with hooks** (what `main_with` does for each run and session invocation):

```rust
let hooks = Hooks::default().subscribe(recorder).on_end(receipts);
let cfg = Config::load_with(&owner, Some(&project), Some(&policy_source))?;   // fail closed
let mut log = AuditLog::open_anchored(&log_path, &anchors)?;
hooks.attach(&mut log);                                   // before anything is recorded
let frontier = OutboundGate::new(log) /* .with_filter(..).with_check(..) */.wrap(provider);
// … RunStart, declass_agent::run(..) or Session::open(..)/turn(..)/end(..) …
declass_agent::conclude_with(&run_dir, &run_id, Some(frontier.audit()), &log_path, &terminal,
    &stats, &Ending { kind: RunKind::Run, mode, resumed, policy: cfg.policy().map(Policy::meta),
    hooks: &hooks })?;
```

Stability. The embedding API is the items named in this section: `declass_cli::{main_with, Embedding,
Product, DoctorCheck, DoctorContext, Check, Status}`; `declass_config::policy` and
`Config::load_with`/`policy`/`policy_rule`/`policy_overrides`/`allows`, `ConfigError::Policy` and
`PolicyLoad`, `Origin::Policy`; `declass_boundary::audit::{AuditSubscriber, Opened, Appended,
Recorded, ChainHead}`, `AuditLog::subscribe`, `AuditHandle::subscribe`/`chain_head`; and
`declass_agent::embed` with `conclude_with`. Declass's crates are versioned together (0.x, not yet on
crates.io); an embedding program pins a Core release. Within 0.x a breaking change to these items
comes with a minor version bump and a release note; a patch release never changes them. Additive
changes are not breaking and may come in any release: new settings (a policy can then name them),
new `AuditEvent` variants (match with a wildcard arm; `AuditEvent::kind` names any), new fields of
the `#[non_exhaustive]` types (`Opened`, `Appended`, `Recorded`, `ChainHead`, `EndReport`,
`EndStats`, `RunKind`, `PolicyMeta`, `DoctorContext`, `Check`, which Core constructs; build a
`PolicyMeta` with `PolicyMeta::new` and a `Check` with `Check::new`), new `Embedding` methods and
new commands or flags of the command line. `Ending` is built by the embedding program; a new field
in it is a breaking change. What a policy can express is the settings that exist: for example no setting forbids
adding MCP servers (each has its own `enabled`), so a policy cannot yet either.
