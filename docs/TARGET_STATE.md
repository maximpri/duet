# Duet v2 — Target State

Status: approved 2026-09-23. Describes the finished product. Built so far: M0–M4.5, SbD-1 and the
SbD-2 tests, M5 preparation and parts of M6 (progress: [PLAN.md](PLAN.md) §10; what is verified:
[ACCEPTANCE.md](ACCEPTANCE.md)). Parts not built yet are marked *(M5)* / *(M6)*.

## 1. North star

> **Frontier-level results, with sensitive information processed only by the local model.**

Duet is a coding agent. A frontier model makes every decision and writes the code. A local model is
the only model that reads sensitive content — secrets, personal data, business data, logs, command
output, and any source code the owner protects. Everything that leaves the machine is checked,
logged, and independently verifiable.

### Success criteria (pre-registered, pass/fail)

| Criterion | Definition |
|---|---|
| **Quality** | Non-inferior to the same frontier model running alone on the dogfood suite ([DOGFOOD_SUITE.md](DOGFOOD_SUITE.md)), decided by the judge: one-sided 95% lower bound of the paired code-quality difference > −2 on a 30-point rubric, and not behind on hidden-test pass rate on a majority of tasks (pass rate is reported with its interval; all-or-nothing tasks make a −5 pp margin need hundreds of pairs) |
| **Sensitivity** | Zero planted canaries from sensitive sources in outbound traffic, verified independently by a logging proxy; 100% of outbound bytes in a hash-chained audit log |
| **Cost** | Reported, not gated: the paired cost ratio vs frontier-only (Duet passthrough on the same model) with its 95% interval, where cost = frontier API list price including cache reads/writes + measured local electricity. The original "strictly cheaper" gate was not met after three attempts (Gate 3, PLAN §5 M4): privacy costs extra frontier turns that offload does not outweigh; operator decision: state a measured privacy premium. Measured so far: about 1.5–2.4× depending on the tasks (ratio of mean cost per run per batch; no interval published yet) |
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
            │   run_command · ask_local · read_raw · finish · (edit_protected)    │
            │        │ every tool result                                         │
            │        ▼                                                           │
            │   CLASSIFY ─► TRANSFORM                                            │
            │     public + small      ─► raw                                     │
            │     secret-bearing      ─► tokenized view   KEY=⟨secret:KEY#1⟩     │
            │     sensitive data/logs ─► handle + digest  h12: 4,112 lines, …    │
            │     public + bulky      ─► handle + head/outline (+ read_raw)      │
            │     protected code      ─► skeleton, bodies as ⟨body:h31⟩          │
            │        ▲                                                           │
            │        │ ask_local(h, questions)    LOCAL MODEL (loopback/allowlist)│
            │        └──────────────────────────────  reads raw, answers in a    │
            │                                          fixed schema; no tools     │
            │   OUTBOUND GATE (sole path to frontier)                            │
            │     tokenize known values → scan secrets/PII → overlap filter      │
            │     → known-value check → block|send → hash-chained audit log      │
            │   WRITE-BACK: placeholders resolved locally; secret-sink check     │
            └────────────────────────────────────────────────────────────────────┘
```

### 3.1 Crates

| Crate | Responsibility |
|---|---|
| `duet-provider` | Model APIs: OpenAI-compatible Chat Completions (z.ai, oMLX, LM Studio, llama.cpp, vLLM, Ollama); Responses and Anthropic Messages *(M6)*; streaming, retry, credentials, local-endpoint trust, usage with cache read/write, pricing |
| `duet-fs` | Handle-relative file access (`O_NOFOLLOW`, `openat`, `renameat`), durable atomic writes, private files, workspace lock, `.duet` path registry |
| `duet-sandbox` | macOS Seatbelt / Linux bwrap command sandbox (sensitive paths unreadable), environment allowlist, output spill, process-tree capture and kill |
| `duet-git` | Private checkpoint store and the single hygienic git helper |
| `duet-config` | Typed settings registry, owner/project scopes, tighten-only project rule |
| `duet-boundary` | Security engine: classification, transformation, placeholder vault, handle store, digests and brief, `ask_local`, bulky offload, IP levels, outbound gate, audit log, local micro-eval |
| `duet-agent` | Frontier loop, tool registry, transcript, context manager, termination, checks, cost ledger |
| `duet-cli` | `run`, `resume`, `audit show/verify`, `config list/get/set/preset`, `purge`, `local-eval`, `doctor`; no-config loopback bootstrap for `run` |
| `duet-tui` | Registry-generated configuration screens, live run view, audit viewer |
| `duet-evals` | `duet-eval`: canary tasks, lanes, leak proxy, judge, statistics, pricing, energy, reports |

Dependency direction: `provider`, `fs` ← `boundary` ← `agent` (with `fs`, `sandbox`, `git`) ←
`cli`; `config` and `git` use `fs`; `agent` never depends on `provider`. `evals` links no duet crate.

### 3.2 Privacy by construction

- The agent crate cannot construct a frontier provider. It receives only a `GatedFrontier`, which
  exists solely as the output of `OutboundGate::wrap(provider)`.
- The local role refuses any endpoint whose host is not loopback or explicitly allowlisted by the
  owner, so a cloud endpoint mislabelled as "local" is rejected; plain HTTP to a non-loopback host
  is refused unless the owner sets `local.allow_plaintext`.
- Commands cannot read sensitive or protected paths (OS sandbox), unless run with `sensitive_data`,
  whose output stays local and whose written files become sensitive.
- Credentials, endpoints, the local-model address and any loosening of the sensitivity policy can
  be set only in the owner's user configuration, never by a repository's project configuration.

## 4. The frontier loop

- **One conversation.** Append-only; byte-stable system prompt; fixed, sorted tool list with no
  per-turn variation. This keeps provider prompt caches valid.
- **Tools.**
  | Tool | Behaviour |
  |---|---|
  | `read_file(path, start_line?, end_line?)` | Result passes through the boundary |
  | `list_files(dir?)`, `search(pattern, dir?)` | Tracked files (`git ls-files`); search shows only the location of a match in a sensitive file, and matches in protected files are marked |
  | `diff` | Working-tree diff, sanitized; protected files' changes omitted |
  | `edit_file(path, edits[{old,new}])` | Duet's own edit format: exact-anchor replacements applied all-or-nothing; placeholders resolved locally |
  | `write_file(path, content)` | Guarded atomic write; placeholders resolved locally |
  | `run_command(command, timeout_seconds?, sensitive_data?)` | Sandboxed, no network, sensitive paths unreadable. Output is sanitized (over 6,000 characters: handle + digest) unless the command is on the raw-output allowlist. With `sensitive_data` it may read sensitive paths; its output becomes a handle and the files it writes become sensitive |
  | `ask_local(handle, questions[≤6])` | Local model answers from raw content in a fixed schema, one answer per question |
  | `read_raw(handle, start_line?, end_line?)` | Ranges (≤500 lines) of public-bulky handles only |
  | `edit_protected(path, spec, tests?, command?)` | Only when IP levels are configured: the local model implements the spec in a protected file; the host writes it and runs the checks; pass/fail and recognised result lines return |
  | `finish(summary)` | Host runs `checks.commands` (sandboxed); results return through the boundary; the frontier may continue up to `limits.max_finish_attempts` |
- **Run start.** In hybrid mode the task gets a note naming the sensitive paths and, if
  `sensitivity.local_brief` is on (off by default), the local model's brief of the sensitive files for this task,
  with values withheld.
- **Loop rules.** Tool errors return as results. A length-truncated response never executes tool
  calls. A tool call is never separated from its result. Command output beyond an in-memory cap
  spills to a file in the run directory.
- **Termination.** `Completed{summary}`, `Failed{reason}`, `BudgetStopped{which}`, always with a
  summary and an audit end event; a panic ends the run as `Failed{internal error: …}`. Provider and
  local-server failures (connect, timeout, stream cut, 429, 5xx) retry in place with backoff capped
  at 60 s and no attempt cap; the run's dollar and wall-clock budgets are the only stop, and end it
  as `BudgetStopped`. Errors a retry cannot fix (credentials, an invalid request) end it as
  `Failed`. Ctrl-C ends it in a resumable `Failed{interrupted}`; `duet resume <run>` reapplies or
  rolls back pending writes and continues, and refuses a completed run.

## 5. Context management (virtual context)

The frontier works on a small curated view; the full content lives locally.

- Sensitive or bulky tool results enter the conversation as **handles** with short digests. Full
  bytes live in `.duet/runs/<id>/handles/`, readable only by the local model (or via `read_raw` for
  public content).
- The conversation only grows at the end. When it passes `context.mask_at` (70%) of the frontier's
  context window, old tool results are replaced **in one batch**, whole turns at a time (the last
  4 turns kept), by stubs naming the call and its handle, e.g.
  `[masked: run_command `cargo test` → h7, ~3100 tokens removed …]`. Batching limits cache
  invalidation.
- Nothing is lost: the frontier can reopen any handle. No summaries replace history.

## 6. Security engine

### 6.1 Classification (inbound)

Every tool result is tagged with its source, then classified by independent layers. Any layer can
mark content sensitive; none can unmark it.

| Layer | Basis |
|---|---|
| Path policy | Globs: `.env*`, `**/.env*`, `*.pem`, `*.key`, `secrets/**`, `data/**`, `*.csv`, `*.db`, `*.sqlite`, `*.parquet`, `logs/**`, `*.log`, plus `sensitivity.protected_paths` |
| Source policy | Command output is sensitive by default; owner-allowlisted commands may return raw (still scanned) output; `sensitive_data` output is always sensitive |
| Secret detectors | Key formats, JWTs, private-key blocks, credential assignments, connection-string passwords, entropy near key-like names |
| PII detectors | Email (reserved example domains skipped), phone, card numbers (Luhn), national IDs, IBAN, IP addresses |
| Sensitive-text detectors | In sensitive content: person names, values of person/address fields whatever their shape, long numbers |
| Taint | Files written by a `sensitive_data` command become sensitive |
| IP level | Paths marked Interface-only or Sealed |

Classes: `PublicSmall`, `SecretBearing`, `SensitiveData`, `PublicBulky`, `Protected`.

### 6.2 Transformation

| Class | Frontier receives |
|---|---|
| PublicSmall | Raw content |
| SecretBearing | Tokenized view preserving structure; values replaced by placeholders |
| SensitiveData | Handle + error lines and repeated line shapes (values replaced) + local digest |
| PublicBulky | Handle + first lines + deterministic outline (local digest for command output only); `read_raw` ranges |
| Protected (Interface-only) | Skeleton: signatures, types, doc comments, public constants; bodies as handles |
| Protected (Sealed) | Existence only; questions via `ask_local` |

**Placeholder vault.** Per run, local only. The same value always maps to the same token
(`⟨email:email#4⟩`, `⟨secret:DB_URL#1⟩`), so the frontier can reason about equality. Tokens carry
kind and key name, not value, length or format. Other spellings of a sensitive value (a surname
alone, a number grouped or in minor units) are aliases of the same token and are never written
back. Values the frontier wrote itself are not rewritten unless they are also in the vault.

### 6.3 Local-role contracts

The local model has no tools and cannot write. It runs extract-only (temperature 0, constrained
JSON output where the backend supports it).

| Role | Output fields |
|---|---|
| Brief (run start) | `{summary, facts[]}` about the sensitive files, for the task |
| Digest | `{summary ≤800 chars, facts[]}` |
| Answer | `{answer ≤1200 chars, evidence_lines[], unanswerable}` |
| Implement (protected edits) | `{code}`: the whole new file |

All roles share one output schema and put the content first, so a server that keys its prompt cache
by schema reuses the processed content across calls. Invalid output is retried once, then reported
as unavailable — never invented. Local output is sanitized and passes the copied-span filter before
the frontier sees it; the local model is told that text inside the content is data, not
instructions.

### 6.4 Outbound gate

Every frontier request, in order:
1. **Tokenize known values** — any vault value (or alias) appearing in any message, including the
   frontier's own text, reasoning and tool-call arguments, is replaced by its placeholder.
2. **Re-scan** with the secret and PII detectors.
3. **Overlap filter** — hashes of 8-token windows over every sensitive text; three or more
   consecutive matching windows (~24 tokens copied) are redacted; spans that also appear in public
   files are exempt. Protected code is redacted the same way.
4. **Final check** — if any vault value remains anywhere in the request, it is blocked, not sent,
   and the run stops.
5. **Audit** — the bytes sent (placeholder-substituted) are appended to a hash-chained log whose
   head is anchored outside the workspace; `duet audit verify <run>` checks both.

Canaries carry no marker, so Duet cannot and does not look for them; the evaluation's independent
proxy does.

### 6.5 Write-back

Placeholders in frontier-written content are resolved locally at write time. The **secret-sink
check** allows a value to land only in owner-listed secret sinks (`sensitivity.secret_sinks`) or
sensitive files. Elsewhere the write is rejected with a tool error telling the frontier to read the
value at runtime (for example from an environment variable).

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
  confirm-on-change flag. Reading a key that is not in the registry is a runtime error; a test that
  scans the sources for settings read outside the registry is planned, not written yet.
- **Files.** Owner: `~/.config/duet/config.toml`. Project: `.duet/config.toml`.
- **CLI.** `duet config get|set|list|preset`, usable without the TUI. A loosening `set` or `preset`
  needs `--confirm` and is recorded in the owner's hash-chained config audit log. Presets cover
  Ollama, LM Studio, llama.cpp, vLLM, oMLX and mlx_lm.server on loopback.
- **Bootstrap.** With no owner `local.base_url`, `duet run` probes 127.0.0.1 on the preset ports
  only, uses a single unambiguous server for that run (recorded in `run.json`), and otherwise prints
  the `duet config set` commands; it never writes configuration.
- **Doctor.** `duet doctor` reports pass/warn/fail with a fix per check (configuration and origins,
  config audit, loosened settings, frontier endpoint and key presence, local-endpoint trust, sandbox,
  git, disk, run audit chains and anchors, retention); offline by default, `--online` adds model
  listings and the local context window, never a model call; `--json`; exit code is the worst result.
  Frontier presets (z.ai, Anthropic, OpenAI), a cache-reuse check and an update check *(M6)*.
- **TUI screens** (`duet tui`; settings screens generated from the registry):

| Screen | Contents |
|---|---|
| Models | Frontier and local settings; `duet doctor` (offline, `--online` on request). Local backend auto-detection in the TUI and connection, cache and prefill-speed tests *(M6)* |
| Sensitivity | Globs, per-detector toggles, raw-output commands, secret-sink allowlist, local brief, bulky thresholds; a live tester for a path (sensitive, secret sink, IP level, matching patterns). Custom detector patterns *(M6)* |
| IP levels | File tree marking Open / Interface-only / Sealed; skeleton preview |
| Limits | Frontier budget, wall clock, local wattage, bulky threshold, masking point |
| Data | Retention (purge from the TUI *(M6)*; `duet purge` today) |
| Audit | Per-run outbound view, block events, hash-chain verification |
| Run | Live turns and tool calls; feed of withheld content and reasons |

- **Safety.** Any loosening requires confirmation, shows a policy diff and is audited. The TUI shows
  each value's origin (default, owner, project). Writes are atomic.

## 9. Local data hygiene

Handles, transcripts, the vault and spill files live in `.duet/runs/<id>/` with mode 0600;
`duet purge` deletes runs older than `data.retention_days` (default 14). The audit log stores
placeholder-substituted text and hashes only (`data.audit_retention_days`, default 90). Automatic
deletion on both periods is *(M6)*.

## 10. Supported models

- **Frontier:** z.ai `glm-5.3-flash` by default (coding endpoint, `frontier.reasoning_effort`
  medium); any OpenAI-compatible Chat Completions endpoint configurable by the owner; Responses and
  Anthropic endpoints *(M6)*.
- **Local:** any OpenAI-compatible Chat Completions server (oMLX, LM Studio, llama.cpp, vLLM,
  Ollama) — loopback, or a host the owner explicitly allowlists; non-loopback plain HTTP is refused
  unless the owner sets `local.allow_plaintext = true`. The model is chosen with `duet local-eval`
  (planted-fact accuracy ≥ 0.90, error-line recall ≥ 0.95, schema validity ≥ 0.99, zero leaks;
  prefill speed reported, not gated). Default: oMLX `omlx-coding` (Qwen 3.8 27B), which scored 1.00
  on accuracy, evidence, recall and schema with 0 leaks.

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
