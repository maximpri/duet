# Duet v2 Architecture

Status: design. Nothing described here is implemented yet. For goals and success criteria see
[docs/TARGET_STATE.md](docs/TARGET_STATE.md); for sequencing see [docs/PLAN.md](docs/PLAN.md).

## 1. Shape of the system

Duet runs one coding task as one continuous conversation with a **frontier model**. The frontier
decides every step and writes all code in Open paths. A **local model** on the same machine is the
only model that ever reads sensitive content; it answers questions and writes digests, and never
acts. Between the two sits the **boundary**, and between Duet and the network sits a single
**outbound gate**.

```
                      ┌─────────────────────────────────────────────┐
                      │                duet-agent                   │
  task ─────────────► │  Loop ── ToolRegistry ── ContextManager     │
                      │   │            │               │            │
                      │   │      tool results          │ masking    │
                      │   │            ▼               │            │
                      │   │     ┌──────────────────────┴────────┐   │
                      │   │     │         duet-boundary          │   │
                      │   │     │ Classifier → Transformer       │   │
                      │   │     │ Vault · HandleStore · Digester │◄──┼── LocalModel (loopback)
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
duet-fs   duet-sandbox   duet-git   duet-config        (leaf crates)
     \          |           |          /
      └─────────┴─── duet-provider ───┘
                         │
                    duet-boundary
                         │
                     duet-agent
                    /          \
               duet-cli      duet-tui

duet-evals depends on duet-provider (pricing, usage types) and drives the duet binary as a
black box; it does not link duet-agent.
```

| Crate | Owns | Must never |
|---|---|---|
| `duet-provider` | Wire dialects (Chat Completions, Responses, Anthropic Messages), streaming assembly, retry, credentials, `Usage`, `Price` | Know about tools, policy or the boundary |
| `duet-fs` | `PinnedParent` handle-relative I/O, atomic durable writes, private files, workspace lock, spill store, `.duet` path registry | Open a workspace path by string after validation |
| `duet-sandbox` | Seatbelt/bwrap profiles, env allowlist, process-tree capture and kill | Decide what a command is allowed to mean (no refusal logic) |
| `duet-git` | Private checkpoint store; the only function that spawns `git` | Inherit the user's git config, hooks or fsmonitor |
| `duet-config` | Settings registry, file loading, scope and tighten-only rules | Accept owner-only keys from a project file |
| `duet-boundary` | Classification, transformation, vault, handles, digests, local roles, outbound gate, audit | Expose a way to reach the frontier without the gate |
| `duet-agent` | Loop, tools, transcript, context manager, termination, cost ledger | Construct a frontier provider (it receives `GatedFrontier`) |
| `duet-cli` / `duet-tui` | User interfaces over config, runs and audits | Contain policy logic (they edit the registry) |
| `duet-evals` | Tasks, canaries, leak proxy, judge, statistics, reports | Share code paths with the product's privacy decisions |

## 3. Key types

```rust
// duet-provider
trait Provider { async fn create(&self, req: &Request, on: &mut dyn FnMut(&StreamProgress))
                    -> Result<Response, ProviderError>;
                 fn identity(&self) -> ModelIdentity; }
struct Usage { input, cache_read, cache_write, output, reasoning: u64, status: UsageStatus }
enum UsageStatus { Reported, Estimated, Unknown }

// duet-boundary
enum Source { File { path }, Command { argv, exit }, Search { pattern }, Git { argv },
              LocalAnswer { handle } }
enum Class  { PublicSmall, SecretBearing, SensitiveData(Reason), PublicBulky,
              Protected(IpLevel) }
enum IpLevel { InterfaceOnly, Sealed }
struct HandleId(u32);                    // rendered "h12"
struct Placeholder { kind, key, n }      // rendered "⟨secret:DB_URL#1⟩"
struct View { class: Class, body: ViewBody, handle: Option<HandleId> }   // what the frontier gets
struct OutboundGate;  fn wrap(p: Box<dyn Provider>) -> GatedFrontier
struct GatedFrontier; // only type the agent can call the frontier through

// duet-agent
trait Tool { fn name(&self) -> &'static str; fn schema(&self) -> ToolSchema;
             fn run(&self, args: Value, ctx: &mut Ctx) -> ToolResult; }
enum Terminal { Completed { checks }, Failed { reason }, BudgetStopped { which } }
```

## 4. Lifecycle of one turn

```
1. ContextManager builds the request: fixed system prompt + fixed sorted tools + transcript
   (masked stubs for old observations once over ~70% of the window).
2. GatedFrontier.create(request)
     OutboundGate: tokenize vault values → secret/PII scan → overlap filter → canary check
                   → (hit: replace, re-check) → append AuditLog record → Provider.create
3. Response parsed. If stop = length/filter: record, no tool execution.
4. For each tool call, in order:
     a. validate arguments against the tool schema (errors return as tool results)
     b. resolve placeholders in arguments (write tools only), secret-sink check
     c. execute (duet-fs / duet-sandbox / duet-git)
     d. Boundary: classify(source, bytes) → transform → View
     e. append call + View to Transcript (synced)
5. Loop until finish passes checks, budget stops, or an unrecoverable failure.
```

## 5. Boundary internals

### 5.1 Classification

Layers run independently; the result is the most restrictive class any layer assigns.

| Layer | Input | Signals |
|---|---|---|
| Path | `Source::File` path | sensitive and protected globs |
| Source | command / git argv | command output and git history sensitive unless allowlisted |
| Secret detector | bytes | key formats, JWT, private keys, credential assignments, connection strings, entropy |
| PII detector | bytes | email, phone, card (Luhn), national IDs, IBAN, IP |
| Local assist | data files, logs | spans patterns miss (names, addresses, free text) |
| Taint | handle lineage | anything derived from a sensitive handle stays sensitive |
| IP | path | Interface-only / Sealed marks |

### 5.2 Transformation

| Class | View body |
|---|---|
| PublicSmall | raw bytes |
| SecretBearing | tokenized copy: structure intact, values → placeholders |
| SensitiveData | handle + digest (deterministic prepass: size, exit code, error lines; local summary; redaction) |
| PublicBulky | over `sensitivity.bulky_tokens` (chars/3): handle + first 40 lines + deterministic outline (declarations; error/warning lines and last 20 lines of output; counts per directory/file for listings and searches) + local summary of files and output; `read_raw` ranges (source line numbers, ≤500 lines, scanned) |
| Protected(InterfaceOnly) | tree-sitter skeleton; bodies → `⟨body:hN⟩` |
| Protected(Sealed) | existence only |

Metadata: sensitive paths render as `dir/⟨file:hN⟩.ext`; search over sensitive files returns counts
only; git history is sensitive; the `diff` tool covers public files only.

### 5.3 Local roles

The local model is called only by the boundary, never by the loop directly, and has no tools.

| Role | Trigger | Output schema |
|---|---|---|
| Digest | new SensitiveData / PublicBulky handle | `{summary, key_lines[], counts, errors[], confidence}` |
| Answer | `ask_local(handle, question)` | `{answer, evidence_lines[], unanswerable}` |
| PII flag | data file or log classification | `{spans[]}` |
| Implement | `edit_protected(path, spec, tests?, command?)` | `{code}`: the whole new protected file, written by the host and validated by host-run checks |

All roles share one output schema (servers may key their prompt cache by schema); each role checks
its own required fields.
Temperature 0; constrained JSON where the backend supports it; one retry, then `unanswerable`.
Every local output is redacted and passes the overlap filter before it can enter a View, and is
wrapped as `{"local_answer", "source"}`.

### 5.4 Outbound gate

The gate is a pure function over the serialized request plus the run's vault and handle index,
followed by an append to the audit log.

- **Overlap index:** rolling hashes of 8-token windows for every sensitive handle; a hit is ≥3
  consecutive windows not also present in public files.
- **Blocking is not an error path for the user:** flagged spans are replaced and the request is
  re-checked; the frontier never sees the original.
- **Audit record:** `{seq, prev_hash, request_sha256, bytes (placeholder-substituted), blocked[],
  provider, model, terms}`; `duet audit verify` recomputes the chain.

### 5.5 Write-back

Write tools receive frontier text containing placeholders. The vault resolves them locally. Each
placeholder has an origin; it may resolve only into its origin file, other secret-glob files or
owner-listed config. Elsewhere, known languages get an environment-variable reference; otherwise
the tool returns an error explaining the rule.

## 6. Context management

- Transcript is append-only and is the source of every request, so the provider prefix stays
  stable across turns.
- Token accounting uses the provider's native count where available, estimate otherwise.
- Above ~70% of the frontier window, tool results are replaced at once, whole turns at a time
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
  runs/<run-id>/              mode 0700, files 0600, retention_days
    transcript.jsonl          full conversation items, synced per item
    handles/<hN>              raw bytes of handles (local only)
    vault.json                placeholder ↔ value map (local only)
    spill/                    long command outputs
    writes.jsonl              pending/applied records for crash recovery
  audit/<run-id>.jsonl        hash-chained outbound log (placeholder-substituted)
~/.config/duet/config.toml    owner settings (credentials, endpoints, local address, policy)
```

## 8. Safety primitives

- **File I/O:** all workspace access goes through `PinnedParent` (`O_NOFOLLOW` walk, `openat`,
  `renameat`, `fsync`); writes are atomic with digest preconditions and rollback; `.git` and
  `.duet` are never writable by tools.
- **Commands:** Seatbelt (macOS) or bwrap (Linux) with absolute binary paths; workspace-write with
  `.git`/`.duet` denied; network off unless allowed; tmpfs `/run`; restricted service lookup;
  process-tree kill on timeout or interrupt.
- **Git:** one helper; environment cleared; fsmonitor, hooks, filters and user/system config
  disabled.
- **Credentials:** owner config only; resolved in a cleared environment with a timeout and size cap;
  never written to transcripts or audit.

## 9. Configuration

`duet-config` holds a registry of every setting:

```rust
struct Setting { key, ty, default, range, scope: Owner | Project,
                 direction: TightenOnly | Any, help, confirm: bool }
```

Loading merges defaults → owner file → project file, rejecting owner-only keys in project files
and any project value that loosens privacy. The CLI (`duet config`) and the TUI are both generated
from the registry; a test fails if code reads a setting that is not registered.

## 10. Termination and recovery

- Provider and local-server failures retry in place with backoff and a wall-clock bound.
- `length` / `content_filter` stops never execute tools.
- Writes record `pending` before and `applied` after; `duet resume <run>` reconciles pending writes
  and continues the transcript.
- Ctrl-C completes the in-flight write and ends in resumable `Failed{interrupted}`.
- Budgets (frontier dollars, wall clock) end in `BudgetStopped`.

## 11. Evaluation architecture

`duet-eval` treats every agent — Duet in its three modes and external agents — as a black box
behind a **logging reverse proxy** that sits between the agent and its frontier endpoint.

```
task workspace (with canaries) ──► agent lane ──► leakproxy ──► frontier API
                                                     │
                                                leaks.jsonl  (independent of Duet's audit log)
workspace ──► sealed grader (hidden tests, secret-sink scan) ──► judge (code quality) ──► stats ──► report
```

Tasks come from the tiered dogfood suite (docs/DOGFOOD_SUITE.md). Gates use paired designs and
pre-registered margins (hidden-test pass rate −5 pp and code quality −2 non-inferiority, cost < 0
superiority, zero leaks).

## 12. Invariants (each backed by a test)

1. No code path reaches the network with a frontier request except through `OutboundGate`.
2. No vault value, canary or ≥24-token span of a sensitive handle appears in any audit record.
3. The local provider's endpoint is loopback or owner-allowlisted.
4. Project configuration cannot loosen privacy or set owner-only keys.
5. The system prompt and tool list are byte-identical for every turn of a run.
6. A tool call is never persisted or sent without its result.
7. Every run ends in a `Terminal` state; no run exits with pending writes unrecorded.
8. `.git` and `.duet` are not writable by tools or sandboxed commands.
9. No file under `crates/` contains another coding agent's code, format or name (outside eval lane
   adapters).
