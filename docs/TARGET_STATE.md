# Duet v2 — Target State

Status: design, approved 2026-09-23. Implementation has not started.
Companion document: [PLAN.md](PLAN.md).

## 1. North star

> **Frontier-level results, with sensitive information processed only by the local model.**

Duet is a coding agent. A frontier model makes every decision and writes the code. A local model is
the only model that reads sensitive content — secrets, personal data, business data, logs, command
output, and any source code the owner protects. Everything that leaves the machine is checked,
logged, and independently verifiable.

### Success criteria (pre-registered, pass/fail)

| Criterion | Definition |
|---|---|
| **Quality** | Non-inferior to the same frontier model running alone on the dogfood suite ([DOGFOOD_SUITE.md](DOGFOOD_SUITE.md)): hidden-test pass rate within 5 percentage points (one-sided 95% lower bound of the paired difference > −5 pp); secondary code-quality score within 2 points on a 30-point rubric |
| **Sensitivity** | Zero planted canaries from sensitive sources in outbound traffic, verified independently by a logging proxy; 100% of outbound bytes in a hash-chained audit log |
| **Cost** | Strictly cheaper than frontier-only: paired upper 95% bound of (duet − frontier-only) cost < 0, where cost = frontier API list price including cache reads/writes + measured local electricity |
| **Termination** | Every run ends as `Completed`, `Failed{reason}` or `BudgetStopped`; infrastructure failures retry in place; interrupted runs are resumable |
| **IP** | Zero IP canaries (protected function bodies) in outbound traffic; the quality cost of protected-code edits is measured and published |

### Who it is for

People who want frontier-quality coding help on repositories that contain things they cannot or
will not send to a cloud model: credentials, customer data, production logs, regulated data, or
crown-jewel source code. Today they choose between a frontier agent that sees everything and a
local-only agent with lower quality. Duet removes that choice for the sensitive parts.

## 2. Principles

1. **The frontier decides, the local model reads.** Quality follows whichever model makes the
   per-turn decisions, so the frontier makes them. The local model never acts; it digests and
   answers.
2. **Withhold, don't obfuscate.** Sensitive bytes never reach the frontier; it receives structure,
   placeholders, handles and answers. Identifier renaming or other obfuscation is not used.
3. **One door out.** A single outbound gate is the only code path to the frontier, enforced by the
   type system.
4. **Prove it, don't claim it.** Every privacy claim is measured with canaries and an independent
   proxy; every quality and cost claim is measured with pre-registered statistics.
5. **Minimal loop.** One continuous conversation, a fixed tool set, no phases, no pass limits, no
   host refusals beyond the sandbox.
6. **Novel.** No code, formats, prompts, tool schemas or design documents from any other coding
   agent. Rust only. GPL-3.0.
7. **Everything configurable.** Every setting lives in one typed registry; the TUI and CLI are
   generated from it.

## 3. Architecture

```
            ┌──────────────────────────── duet run ─────────────────────────────┐
 user task ─► FRONTIER LOOP  (one append-only conversation, fixed sorted tools)   │
            │   read_file · list_files · search · diff · edit_file · write_file   │
            │   run_command · ask_local · read_raw · finish                       │
            │        │ every tool result                                         │
            │        ▼                                                           │
            │   CLASSIFY ─► TRANSFORM                                            │
            │     public + small      ─► raw                                     │
            │     secret-bearing      ─► tokenized view   KEY=⟨secret:KEY#1⟩     │
            │     sensitive data/logs ─► handle + digest  h12: 4,112 lines, …    │
            │     public + bulky      ─► handle + digest  (+ read_raw ranges)    │
            │     protected code      ─► skeleton, bodies as ⟨body:h31⟩          │
            │        ▲                                                           │
            │        │ ask_local(h, question)        LOCAL MODEL (loopback only)  │
            │        └──────────────────────────────  reads raw, answers in a    │
            │                                          fixed schema; no tools     │
            │   OUTBOUND GATE (sole path to frontier)                            │
            │     tokenize known values → scan secrets/PII → overlap filter      │
            │     → canary check → block|send → hash-chained audit log           │
            │   WRITE-BACK: placeholders resolved locally; secret-sink check     │
            └────────────────────────────────────────────────────────────────────┘
```

### 3.1 Crates

| Crate | Responsibility |
|---|---|
| `duet-provider` | Model APIs: OpenAI-compatible Chat Completions (z.ai, LM Studio, llama.cpp, vLLM), Responses (Ollama, oMLX, OpenAI), Anthropic Messages; streaming, retry, credentials, usage with cache read/write, pricing |
| `duet-fs` | Handle-relative file access (`O_NOFOLLOW`, `openat`, `renameat`), durable atomic writes, private files, workspace lock, spill store, `.duet` path registry |
| `duet-sandbox` | macOS Seatbelt / Linux bwrap command sandbox, environment allowlist, process-tree capture and kill |
| `duet-git` | Private checkpoint store and the single hygienic git helper |
| `duet-config` | Typed settings registry, owner/project scopes, tighten-only project rule |
| `duet-boundary` | Security engine: classification, transformation, placeholder vault, handle store, digests, `ask_local`, outbound gate, audit log |
| `duet-agent` | Frontier loop, tool registry, transcript, context manager, termination, checks, cost ledger |
| `duet-cli` | `run`, `resume`, `audit`, `purge`, `config`, `doctor`, `setup` |
| `duet-tui` | Registry-generated configuration screens, live run view, audit viewer |
| `duet-evals` | `duet-eval`: canary tasks, lanes, leak proxy, judge, statistics, pricing, energy, reports |

Dependency direction: `fs`, `sandbox`, `git`, `config` ← `provider` ← `boundary` ← `agent` ←
`cli`, `tui`. Approximate size: 23K lines of production Rust.

### 3.2 Privacy by construction

- The agent crate cannot construct a frontier provider. It receives only a `GatedFrontier`, which
  exists solely as the output of `OutboundGate::wrap(provider)`.
- The local role refuses any endpoint whose host is not loopback or explicitly allowlisted by the
  owner, so a cloud endpoint mislabelled as "local" is rejected.
- Credentials, endpoints, the local-model address and any loosening of the sensitivity policy can
  be set only in the owner's user configuration, never by a repository's project configuration.

## 4. The frontier loop

- **One conversation.** Append-only; byte-stable system prompt; fixed, sorted tool list with no
  per-turn variation. This keeps provider prompt caches valid.
- **Tools.**
  | Tool | Behaviour |
  |---|---|
  | `read_file(path, start?, end?)` | Result passes through the boundary |
  | `list_files`, `search` | Classified per file; sensitive files appear as `dir/⟨file:hN⟩.ext`; search reports counts only for sensitive files |
  | `diff` | Working-tree diff of public files only |
  | `edit_file(path, edits[{old,new}])` | Duet's own edit format: exact-anchor replacements applied all-or-nothing; placeholders resolved locally |
  | `write_file(path, content)` | Guarded atomic write; placeholders resolved locally |
  | `run_command(argv)` | Sandboxed; output sensitive by default (handle + digest) unless the command is on the raw-output allowlist |
  | `ask_local(handle, question)` | Local model answers from raw content in a fixed schema |
  | `read_raw(handle, start?, end?)` | Raw ranges of public-bulky handles only |
  | `edit_protected(path, spec, tests?, command?)` | Only when IP levels are configured: the local model implements the spec in a protected file; the host writes it and runs the checks; pass/fail and recognised result lines return |
  | `finish(summary)` | Host runs the task's declared checks; results return through the boundary; the frontier may continue up to `max_finish_attempts` |
- **Loop rules.** Tool errors return as results. A length-truncated response never executes tool
  calls. A tool call is never separated from its result. Long outputs spill to files referenced in
  the result.
- **Termination.** `Completed{checks}`, `Failed{reason}`, `BudgetStopped{which}`. Provider and
  local-server failures retry in place with backoff. Ctrl-C finishes the current write and ends in
  a resumable `Failed{interrupted}`; `duet resume <run>` reapplies or rolls back pending writes and
  continues.

## 5. Context management (virtual context)

The frontier works on a small curated view; the full content lives locally.

- Sensitive or bulky tool results enter the conversation as **handles** with short digests. Full
  bytes live in `.duet/runs/<id>/handles/`, readable only by the local model (or via `read_raw` for
  public content).
- The conversation only grows at the end. When it passes ~70% of the frontier's context window, old
  tool results are replaced **in one batch** by their handle stubs, e.g.
  `[masked: h12, 3.1K tokens — ask_local / read_raw]`. Batching limits cache invalidation.
- Nothing is lost: the frontier can reopen any handle. No summaries replace history.

## 6. Security engine

### 6.1 Classification (inbound)

Every tool result is tagged with its source, then classified by independent layers. Any layer can
mark content sensitive; none can unmark it.

| Layer | Basis |
|---|---|
| Path policy | Globs: `.env*`, `*.pem`, `*.key`, `secrets/**`, `data/**`, `*.csv`, `*.db`, `*.sqlite`, `*.parquet`, `logs/**`, `*.log`, owner-protected paths |
| Source policy | Command output and `git log/diff/show` are sensitive by default; owner-allowlisted commands may return raw output |
| Secret detectors | Key formats, JWTs, private-key blocks, credential assignments, connection strings, entropy near key-like names |
| PII detectors | Email, phone, card numbers (Luhn), national IDs, IBAN, IP addresses |
| Local assist | For data files and logs: names, addresses and free text that patterns miss |
| Taint | Anything derived from a sensitive handle inherits its sensitivity |
| IP level | Paths marked Interface-only or Sealed |

Classes: `PublicSmall`, `SecretBearing`, `SensitiveData`, `PublicBulky`, `Protected`.

### 6.2 Transformation

| Class | Frontier receives |
|---|---|
| PublicSmall | Raw content |
| SecretBearing | Tokenized view preserving structure; values replaced by placeholders |
| SensitiveData | Handle + digest; any quoted values replaced by placeholders |
| PublicBulky | Handle + digest; `read_raw` ranges available |
| Protected (Interface-only) | Skeleton: signatures, types, doc comments, public constants; bodies as handles |
| Protected (Sealed) | Existence only; questions via `ask_local` |

**Placeholder vault.** Per run, local only. The same value always maps to the same token
(`⟨pii:email#4⟩`), so the frontier can reason about equality. Tokens carry type and origin, not
value, length or format.

### 6.3 Local-role contracts

The local model has no tools and cannot write. It runs extract-only (temperature 0, constrained
JSON output where the backend supports it).

| Role | Output schema |
|---|---|
| Digest | `{summary ≤800 chars, key_lines[], counts, errors[], confidence}` |
| Answer | `{answer ≤1200 chars, evidence_lines[], unanswerable}` |
| PII flag | `{spans[]: {start, end, kind}}` |

Invalid output is retried once, then returned as "local answer unavailable" — never invented. Local
output reaches the frontier wrapped as `{"local_answer", "source"}` and is declared to be data, not
instructions.

### 6.4 Outbound gate

Every frontier request, in order:
1. **Tokenize known values** — any vault value appearing anywhere is replaced by its placeholder.
2. **Re-scan** with the secret and PII detectors.
3. **Overlap filter** — rolling hashes of 8-token windows over every sensitive handle; three or more
   consecutive matching windows (~24 tokens copied) are redacted; spans that also appear in public
   files are exempt.
4. **Canary check** — in evaluation, any canary blocks the request.
5. **Decide** — anything flagged is replaced and re-checked; a request is never sent with a hit.
6. **Audit** — final bytes (placeholder-substituted) are appended to a hash-chained log;
   `duet audit verify <run>` proves integrity.

### 6.5 Write-back

Placeholders in frontier-written content are resolved locally at write time. The **secret-sink
check** allows a secret to land only in its origin file, other secret-glob files, or owner-listed
config files. Elsewhere it becomes an environment-variable reference when the language is known, or
the write is rejected with a clear tool error.

### 6.6 Residual leakage (stated, not hidden)

- Existence and shape: that a key exists, a file's column count, how many errors a log contains.
- Paraphrase: short local answers can convey meaning the overlap filter cannot detect.
- Open source code is visible to the frontier; mark paths protected to withhold them.
- The task description and skeletons reveal intent and architecture.
- Misclassification of a novel secret format — measured by canaries, not assumed away.

## 7. Intellectual property levels

| Level | Frontier sees | Editing |
|---|---|---|
| **Open** (default) | Source | Normal |
| **Interface-only** | Tree-sitter skeleton; bodies as handles | Frontier writes spec + tests → local model implements the body → host runs tests → pass/fail returns through the boundary |
| **Sealed** | Existence only | Via `ask_local` and local implementation |

Edits under Interface-only and Sealed are local-quality by construction; their quality cost is
reported separately. IP canaries (unique function bodies) must never cross.

## 8. Configuration and TUI

- **Registry.** Every setting is declared once in `duet-config`: key, type, default, range, scope
  (owner-only or project), direction rule (projects may only tighten privacy), help text,
  confirm-on-change flag. A test fails on any setting read outside the registry.
- **Files.** Owner: `~/.config/duet/config.toml`. Project: `.duet/config.toml`.
- **CLI.** `duet config get|set|list`, usable without the TUI.
- **TUI screens** (generated from the registry):

| Screen | Contents |
|---|---|
| Models | Frontier provider/model; local backend auto-detection (Ollama, LM Studio, llama.cpp, vLLM, oMLX); connection, cache and prefill-speed tests |
| Sensitivity | Globs, per-detector toggles, custom patterns with a live tester, raw-output commands, secret-sink allowlist |
| IP levels | File tree marking Open / Interface-only / Sealed; skeleton preview |
| Limits | Frontier budget, wall clock, local wattage, bulky threshold, masking point |
| Data | Retention, purge |
| Audit | Per-run outbound view, block events, hash-chain verification |
| Run | Live turns and tool calls; feed of withheld content and reasons |

- **Safety.** Any loosening requires confirmation, shows a policy diff and is audited. The TUI shows
  each value's origin (default, owner, project). Writes are atomic.

## 9. Local data hygiene

Handles, transcripts and spill files live in `.duet/runs/<id>/` with mode 0600, deleted after
`retention_days` (default 14) or by `duet purge`. The audit log stores placeholder-substituted text
and hashes only (default retention 90 days).

## 10. Supported models

- **Frontier:** z.ai GLM flagship by default; any OpenAI-compatible, Responses or Anthropic endpoint
  configurable by the owner.
- **Local:** Ollama, LM Studio, llama.cpp, vLLM, oMLX — loopback, or a LAN host the owner explicitly
  allowlists (the operator's current setup: oMLX `omlx-coding` on `192.168.50.132:8080`); non-loopback
  plain HTTP triggers a `duet doctor` warning. The default local model is
  chosen by a micro-evaluation (error-line recall ≥ 0.95, planted-fact accuracy ≥ 0.90, schema
  validity ≥ 0.99, prefill ≥ 500 tok/s at 16K context).

## 11. What duet v2 deliberately does not have

Phases, pass limits, host refusals beyond the sandbox, work orders, diff review, an in-product judge
or advisor, escalation, a model router, distributed members, MCP, an HTTP API, RPC, obfuscation.

## 12. Threat model (summary; full version in `SECURITY.md`)

- **Protected:** content classified sensitive or protected, against disclosure to the cloud
  frontier provider.
- **Not protected:** the local machine, the local model server, the operator, source code left
  Open, information in the task description.
- **Assumed:** local backends run on loopback, or on an owner-allowlisted LAN host whose network
  path is trusted (preferably tunnelled or TLS); the owner's configuration is trusted;
  repository content (including project configuration) is untrusted.
