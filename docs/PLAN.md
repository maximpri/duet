# Duet v2 — Implementation Plan

Status: approved 2026-09-23; in progress (Gate 3 of M4; SbD-2 next). Progress: §10. Target state: [TARGET_STATE.md](TARGET_STATE.md).

## 1. Why v2

Duet v1 (`/Users/maximp/OpenCode/duet_agent`, ~310K lines of Rust, 453 commits since 2026-08-12)
missed its goal of frontier quality at lower cost with most work on a local model:

- Same local model: v1's harness scored 18/50, a plain single loop 43/50.
- Best v1 score 39 vs frontier-only 48 in the same blind judging.
- 92% of frontier tokens went to diff review, 1% to planning.
- Cost was never measured; the judge drifted ~7 points on an unchanged file.

Evidence review (see `docs/evidence/`): quality is decided by whichever model makes the per-turn
decisions; a router cannot keep sensitive content out of the context; local execution's real
advantage is privacy, not price. v2 therefore puts the frontier in the decision seat and gives the
local model the job of handling everything sensitive.

v1 is frozen. It is used only as a source of the owner's own code to port, after a provenance check.

## 2. Rules for the whole project

1. **Measure first.** The benchmark exists before the agent. Nothing proceeds past a failed gate.
2. **One variable per measured run.**
3. **Line budgets per crate** (§4). Exceeding one requires an explicit decision.
4. **Novelty and provenance.** No code, formats, prompts, tool schemas or design documents from any
   coding agent. Each v1 file is checked before porting (agent-name/format grep, git history).
   Excluded: v1's patch envelope (`src/tools/patch.rs`), the Codex CLI transport and presets, and v1
   documents comparing other agents. Public API wire formats are protocols and are allowed.
   `tools/gate.sh` fails on agent names or formats in `crates/`, except the eval lane adapters.
5. **Rust only.** No Python anywhere, including the evaluation harness.
6. **License GPL-3.0**, SPDX headers, dependency licenses checked by cargo-deny.
7. **No hosted CI.** `tools/gate.sh` is the gate: fmt, clippy `-D warnings`, `cargo test`,
   cargo-deny, provenance grep, eval self-tests.

## 3. Decisions log

| Date | Decision |
|---|---|
| 2026-09-23 | North star: frontier-level results, sensitive information processed only by the local model |
| 2026-09-23 | Architecture: frontier decides, local reads; real agent loop, not a router, not v1's phase machine |
| 2026-09-23 | Sensitive by default: secrets, `.env`, PII, data files/DBs, logs, command output; source code Open unless marked |
| 2026-09-23 | Cost gate: strictly cheaper than frontier-only |
| 2026-09-23 | Frontier: z.ai `glm-5.3` (flagship; configurable). Eval judge: Claude via Anthropic API |
| 2026-09-23 | Frontier switched to `glm-5.3-flash` (operator, after the coding-plan 5-hour quota was hit); earlier `glm-5.3` pilot data is not comparable and is kept only as reference |
| 2026-09-23 | New repository `/Users/maximp/OpenCode/duet_v2`; v1 frozen |
| 2026-09-23 | Pi, Claude Code, Codex allowed only as black-box evaluation lanes |
| 2026-09-23 | Rust only; fully novel; GPL-3.0 |
| 2026-09-23 | IP levels (Open / Interface-only / Sealed) included in v2 |
| 2026-09-23 | Everything configurable via a registry-driven TUI |
| 2026-09-23 | Judge runs through the logged-in Claude CLI (subscription) instead of an API key; Codex CLI available as an alternative backend |
| 2026-09-23 | Quality gates are decided by the judge (lower bound of the paired difference > −2/30). Hidden pass rate is reported; a pass-rate gap counts only when the candidate is behind on a majority of tasks. Reason: all-or-nothing tasks make the −5 pp pass-rate margin need hundreds of pairs (operator, after Gate 1) |
| 2026-09-23 | Commands cannot read sensitive paths (OS sandbox); a command that must read them runs with `sensitive_data`, its output is held locally and the files it writes become sensitive. Value-based sanitizing of command output cannot stop derived or re-encoded data (hybrid batch, 3 of 5 leaking runs) |
| 2026-09-23 | Local model stays oMLX `omlx-coding` (Qwen 3.8 27B): micro-eval accuracy, evidence, digest recall, schema 1.00, 0 leaks. First-read prefill ~175 tok/s is expected and accepted; prefill is reported, not gated (operator) |
| 2026-09-23 | Cost gate reference confirmed: hybrid vs the orchestrator alone (`duet-passthrough`, same frontier model, currently `glm-5.3-flash`), paired; not vs Pi or another model (operator) |
| 2026-09-23 | The ratatui TUI (all screens, including Run and Audit) stays in M6, after the privacy and cost gates (operator) |
| 2026-09-23 | duet-boundary budget raised from ~3.2K to ~5K production lines (it absorbed M4 bulky offload and M4.5 IP levels; measured ~4.55K); further growth needs another decision (operator) |
| 2026-09-24 | Secure by Design is core for duet (operator): cross-cutting SbD track added (§5); SbD-1 and a red-team pass must pass before the public benchmark (M5) |
| 2026-09-24 | M5 external lanes run on the operator's CLI subscriptions, not API keys (operator): Claude Code via a `claude setup-token` subscription token (`CLAUDE_CODE_OAUTH_TOKEN`) with an isolated config dir; Codex via a dedicated eval `CODEX_HOME` logged in once with ChatGPT (never a copy of the main login: refresh-token rotation). Both must route through the leak proxy; verified by a one-run smoke test per lane before M5 |
| 2026-09-24 | Gate 3 outcome (operator, pre-agreed): not strictly cheaper after attempts A–C, so cost is reported as a measured privacy premium (paired ratio vs the orchestrator alone with its interval, ~1.4×); quality non-inferiority and zero leaks stay required. The local task brief (attempt C) raised cost and is off by default |
| 2026-09-24 | M5 is judged by two judges of different families, both through the operator's CLI subscriptions: Claude (Claude CLI) and OpenAI (Codex CLI). Every run is judged by both; gates use the mean; the report shows each judge and flags runs judged by their own family (operator) |

Open decisions: evaluation budget cap (set after the first pilot runs).

Resolved 2026-09-23: GLM prompt caching works on the z.ai coding endpoint. `glm-5.3` with a
28,546-token prompt sent twice reported `cached_tokens` 0 then 28,544 (cache read $0.26/M vs input
$1.40/M). No fallback frontier is needed.

## 4. Repository layout and budgets

| Path | Contents | Production lines |
|---|---|---|
| `crates/duet-provider` | Model APIs, streaming, retry, credentials, usage, pricing | ~6.5K |
| `crates/duet-fs` | Guarded file access, atomic writes, lock, spill, `.duet` registry | ~1.1K |
| `crates/duet-sandbox` | Seatbelt/bwrap, env allowlist, process-tree control | ~1.0K |
| `crates/duet-git` | Private store, single git helper | ~0.6K |
| `crates/duet-config` | Settings registry, scopes, tighten-only rule, `duet config` | ~0.8K |
| `crates/duet-boundary` | Security engine (incl. bulky offload and IP levels) | ~5K |
| `crates/duet-agent` | Frontier loop, tools, transcript, context manager, ledger | ~3.0K |
| `crates/duet-cli` | CLI commands, setup | ~1.0K |
| `crates/duet-tui` | Registry-generated screens, run and audit views | ~2.5K |
| `crates/duet-evals` | `duet-eval` harness | ~3.5K |
| `tools/gate.sh` | Local gate | — |
| `docs/` | This plan, target state, dogfood suite, `security.txt`, `evidence/`; `BENCHMARK.md` (M5). `ARCHITECTURE.md` and `SECURITY.md` are at the root | — |

Total ≈ 23K lines of production Rust.

## 5. Milestones

Approximate schedule: 5.5 weeks to the public benchmark, then hardening.

### M0 — Repository and benchmark first (days 1–4)

**M0.1 Scaffold**
- Cargo workspace, `rust-toolchain.toml`, `deny.toml`, GPL-3.0 `LICENSE`, `tools/gate.sh`.
- Move the v1 research notes and report (untracked in v1: `research_notes/`, `reports/`) into
  `docs/evidence/`.
- `SECURITY.md` threat model (summary in TARGET_STATE §12).

**M0.2 Evaluation harness (`duet-evals`, binary `duet-eval`)** — designed from v1's `evals/dogfood/`,
rewritten in Rust.

| Module | Purpose |
|---|---|
| `task.rs` | Loads task packages (`task.toml`, `objective.md`, `starter/`, `holdout/`, `assets/`, `seal.toml`) per [DOGFOOD_SUITE.md](DOGFOOD_SUITE.md) |
| `grade.rs` | Sealed grading (hidden-test pass rate): verify input seal, stage candidate, run visible and hidden tests sandboxed, scan the final workspace for secret-sink violations; the grade goes into the run record (`run.json`) |
| `judge.rs` | Code-quality rubric: correctness risk, maintainability, scope discipline (3 × 10), versioned by hash; no anchoring. Two judges of different families on the operator's logged-in CLIs (`--judges claude-cli,codex-cli`, the default; `api` replaces the Claude CLI): Claude (`claude-opus-5-5`) and OpenAI via the Codex CLI (`gpt-5.5`), models pinned per gate. Two repeats per artifact per judge, each in an empty temporary directory with structured output; one result file per judge and run; agent names scrubbed and canaries redacted; judge model, CLI version and cost recorded |
| `canary.rs` | Per-run unique, realistic canaries (no marker) in `.env`, data files, logs, a protected path, source code, and non-secret-looking business facts; injection canaries |
| `leakproxy.rs` | Logging reverse proxy (hyper) in front of every lane's frontier endpoint; scans every request body; writes `leaks.jsonl` (independent of Duet's own audit log) |
| `lanes/` | `lanes.toml`: `duet-passthrough` (frontier-only), `duet-hybrid`, `duet-local-only`; black-box external lanes `pi-glm`, `claude-code`, `codex` |
| `pricing.toml` | Verified list prices incl. cache read/write, with source URL and date |
| `cost.rs`, `ledger.rs` | Dollars at list price plus electricity (watts × local busy seconds); Duet's per-run cost ledger read from `summary.json` |
| `stats.rs` | Paired bootstrap by run (10k resamples); non-inferiority bounds on hidden-test pass rate (margin −5 pp, reported) and judge score (margin −2), superiority on cost, exact binomial upper bound on leak rate; judge repeats nested within artifact. The quality verdict is the judge bound plus the per-task majority rule (§3) |
| `report.rs` | Per-lane tables, gate verdict JSON, dashboard |

**M0.3 Dogfood suite** — full design in [DOGFOOD_SUITE.md](DOGFOOD_SUITE.md). Tasks of increasing
complexity, each impossible to complete without reading its sensitive content, graded by hidden
tests:

| Tier | Tasks built in M0 | Later |
|---|---|---|
| S (10–20 min) | S0 `public-refactor` (control, no sensitive content), S1 `config-from-env`, S2 `crash-from-logs` | — |
| M (30–60 min) | M1 `billing-export`, M2 `data-subject-export` (TypeScript), M3 `hostile-logs` (prompt injection) | — |
| L (1–3 h) | L1 `ledger-reconcile` | L2 `pricing-crown-jewel` (M4.5) |
| XL (2–4 h) | — | X1/X2 real open-source repositories with injected sensitive assets (M6) |

**M0.4 Pilot**
- Run `pi-glm`, `claude-code`, `codex` on S0–M3 and L1, 5 runs each, through the leak proxy.
- Calibrate: the frontier-only reference must pass 30–90% of each task's hidden tests; adjust tasks
  outside that band.
- Verify through the proxy that each lane actually opened the sensitive files; otherwise redesign
  the task.
- Verify GLM prompt caching: send one 20K-token prefix twice; record `cached_tokens` and price. If
  caching is absent, flag the cost gate and ask the owner for a fallback frontier.
- Price the evaluation budget from the first 5 runs; the owner sets the cap before continuing.
- Outputs: leak table, GLM baseline cost per task, per-task pass-rate and judge variance → gate
  sample sizes (planning figure: 10 paired runs per task for S and M, 6 for L).

**Acceptance:** `duet-eval selftest` passes (proxy catches a planted canary; judge returns
schema-valid output; every task package validates and its seal verifies; statistics reproduce known intervals); pilot report produced.

### M1 — Provider layer (days 5–8)

Port from v1 `src/provider/mod.rs`, keeping the design where stream assemblers rebuild a body and
one parser per dialect handles both streaming and non-streaming.

| Area | v1 source |
|---|---|
| Errors | `ProviderError` 122–440 |
| Types | `InputItem` 546, `ToolDefinition` 584, request 749 (without `context_snapshot` 788, `deliberation_expected` 814), `ToolCall` 1369, `Usage` 1532, response 1637 + `continuation()` 1735 |
| Client/transport | 2549, 2573, 2718; `Provider` trait 3494–3653 without phase hooks |
| Retry, credentials | 5390–5638, 5735–5873 |
| Wire builders, conversion | 6010–6494, 6727–7164 |
| Streaming | SSE decoder 7282; assemblers 7682, 8103, 8330; consumer 8867 |
| Retry helpers, overflow | 9107–9560, 9577 |
| Parsers, argument repair | 10653–11350 |
| Capacity, quirks | subset of `src/capacity.rs`; `src/backend_quirks.rs` |

Not ported: the Codex CLI transport, goal/session/ledger coupling, v1's duet-specific tool list and
turn contract, oMLX probes, mlx calibration, route cache, request budget.

Fixed while porting (each with a failing-first test):
1. Retry usage poisoning (v1 395, 1522–1528, 5505, 5527, 9423): failed pre-output attempts count
   zero; mid-stream failures estimated; `Unknown` counted separately.
2. Mid-stream error events retried (7702–7707, 8117–8122, 8344–8349).
3. `length`, `max_tokens`, `content_filter` terminal (7614–7623, 8372–8380, 9196).
4. `cache_write` in `Usage`; Anthropic cache creation split out (10835–10841); pricing in Rust.
5. Cache stability: tools sorted on every dialect (6277); no tool dropping on named `tool_choice`
   (921–931); no rewriting of prior reasoning (6983); rolling Anthropic breakpoint.
6. Missing usage on local routes → `Estimated` (9092–9096).
7. Text tool-call recovery for local Chat Completions servers.
8. Native context probes: llama.cpp `/props`, Ollama `/api/show`, LM Studio `/api/v0/models`.
9. Local-role loopback enforcement.

Planned here, run in M3 (§10 scope changes): **local-model micro-evaluation** (`duet local-eval`;
generated logs, CSVs, configs) to choose the local model: error-line recall ≥ 0.95, planted-fact
accuracy ≥ 0.90, schema validity ≥ 0.99, 0 leaks. Prefill speed is reported, not gated (operator,
2026-09-23: slow first reads are expected).

**Acceptance:** `cargo test -p duet-provider`; live tests (`DUET_LIVE_GLM=1 DUET_LIVE_LOCAL=1 cargo
test -p duet-provider -- --ignored live_`) for a tool-call round trip, reported cached tokens and
local prefix reuse.

### M2 — Primitives, configuration, frontier loop in pass-through mode (days 9–15)

Port with fixes:
- **fs:** `src/tools/path_guard.rs` 45–291, `src/state_fs.rs`, atomic write/remove/rollback
  (`src/tools/checkpoint.rs` 1242–1380, `src/tools/mod.rs` 2340–2519), workspace lock
  (`src/lease.rs` 170–360), `src/spill.rs`. All reads handle-relative. `.duet` path registry with a
  test that fails on unclassified `.duet/` literals.
- **sandbox:** `src/tools/shell.rs` 54–131, 209, 429–527, 669–888; `src/process_output.rs`;
  `src/child_env.rs`. Fixes: deny writes to `.git/**` and nested `.git`/`.duet`; bwrap
  `--tmpfs /run`; absolute sandbox binary paths; Seatbelt `mach-lookup` allowlist; kill the whole
  process tree; runtime directories outside child-writable trees; network denied unless allowed.
  Shell-lexer refusal logic is not ported.
- **git:** private store from `src/git_delivery.rs`; one helper for every git call (`env_clear`,
  absolute git, fsmonitor and hooks disabled, global/system config ignored, optional locks off, no
  filters). File listing from `git ls-files` (no file-count cap).
- **tools:** exact/fuzzy replacement (`src/tools/mod.rs` 1266–1327, 3001–3404) and
  `src/workspace_research.rs`, after provenance check. v1's patch format is not ported; v2's
  `edit_file` uses its own `edits[{old,new}]` format.

New:
- **`duet-config`:** registry, owner/project scopes, tighten-only project rule, owner-only keys
  (credentials, endpoints, local address, frontier choice, policy loosening), `duet config`.
- Tool registry and schemas; transcript (full items, synced, capped, torn-tail repair); `run()`
  loop; context manager (native token counts, batch masking at ~70%); terminal states; budgets;
  in-place infrastructure retry; `duet resume`; Ctrl-C → resumable `Failed{interrupted}`.
- Local data hygiene: `.duet/runs/<id>/` 0600, retention, `duet purge`.

The boundary runs in pass-through: the gate audits and scans with an empty policy.

**Gate 1 — harness health.** `duet-passthrough` vs `pi-glm` (same GLM model), paired on S0, S1, S2 and M1:
quality non-inferior by the rule in §3 (judge lower bound > −2/30; not behind on hidden-test pass
rate on a majority of tasks). On failure, only harness work proceeds. Passed on `glm-5.3-flash` (§10).

### M3 — Security engine (days 16–22)

As built (details: ARCHITECTURE §5):
- **Classification:** path and source policy; secret detectors (v1 `src/redact.rs` patterns after
  provenance check, plus missing formats and entropy); PII detectors; name, person/address-field
  and long-number detectors on sensitive text; taint of files written by `sensitive_data` commands.
- **Transformation:** placeholder vault with aliases for other spellings; tokenized views; handle
  store; digests (deterministic error lines and repeated line shapes, then local summary, then
  redaction); priming of every sensitive file at run start; the task names the sensitive paths.
- **Command access:** sensitive paths unreadable to commands (OS sandbox) unless `sensitive_data`
  (output held as a handle, written files become sensitive, persisted across resume).
- **Local-role contracts:** digest and answer fields in one shared schema, content first (prompt
  cache reuse); constrained output where supported; retry once, then unavailable; outputs sanitized
  as sensitive text. `ask_local` takes up to 6 questions per call.
- **Outbound gate:** re-sanitize every message including the frontier's own text and tool-call
  arguments → copied-span filter (8-token windows, ≥3 consecutive matches, public-span exemption) →
  final known-value check (block) → hash-chained audit; `duet audit show|verify`. Canaries are
  found by the leak proxy, not by Duet.
- **Write-back:** local placeholder resolution; secret-sink check.
- **Tests:** unit tests per detector; vault round trip; gate blocking; hash-chain verification;
  injection canaries; end-to-end hybrid runs on S1, S2, M1–M3 and L1 with the proxy. The property
  test that no canary survives the gate is part of SbD-2 (`duet-boundary/tests/no_canary.rs`).

**Gate 2 — privacy without quality loss** (S1, S2, M1, M2, M3, L1). Zero leaks and zero secret-sink
violations across all hybrid runs (leak proxy and grader);
quality non-inferior to passthrough (rule in §3). Passed on `glm-5.3-flash` (§10). Ablations, one at a time: command-output digest vs raw for
allowlisted commands; digest length. If quality fails after two improvement iterations, the claim
becomes "best quality at zero leakage" with the measured cost stated.

### M4 — Bulky offload and cost (days 23–26)

- `PublicBulky` class with head, outline and `read_raw` ranges: outputs, listings and searches over
  `sensitivity.bulky_tokens` (2K); files the model reads over `sensitivity.bulky_file_tokens` (12K);
  no local summary of bulky source. Turn-wise masking with stubs.
- Cost ledger per run (`summary.json`): frontier tokens carried by class (raw, tokenized, handle
  summary, local answer, bulky handle), `ask_local` calls, local busy seconds, dollars at list
  price; `duet-eval report` shows it per lane.
- Local task brief at run start (`sensitivity.local_brief`; attempt C raised cost, so off by default).

**Gate 3 — strictly cheaper** (S0–M3, L1). Paired cost difference vs passthrough (the orchestrator
alone on the same model) with upper 95% bound < 0, with quality still non-inferior and zero leaks.
Attempts A and B failed: privacy forces extra frontier turns (asking about data the frontier cannot
read, e.g. 66 vs 45 requests on L3 in attempt B, ~1.4× the cost), and offload works but does not
outweigh them. Attempt C (local task brief) raised cost further (L3 $0.25 vs $0.18 per run; more
`ask_local` calls, not fewer) and is off by default. **Outcome (operator decision): Gate 3 is a
measured privacy premium** — the paired cost ratio vs the orchestrator alone is reported with its
interval (~1.4× on the measured tasks) instead of a pass/fail gate; quality non-inferiority and zero
leaks remain required.

### M4.5 — IP levels (days 27–31)

- Per-path levels Open / Interface-only / Sealed.
- Tree-sitter skeletons (Rust, TypeScript, Python): signatures, types, doc comments, public
  constants; bodies as `⟨body:hN⟩` handles.
- Protected edits: frontier spec + tests → local implementation → host runs tests → pass/fail back
  through the boundary.
- Overlap filter covers protected code. IP canaries.
- Dogfood task **L2 `pricing-crown-jewel`**.

**Gate:** zero IP-canary leakage. Quality cost of protected edits reported, not gated.

### SbD — Secure by Design (core, cross-cutting)

Duet's product is a security promise: sensitive information never reaches the cloud model. Secure by
Design (CISA and partner agencies' principles and pledge) applies to that promise itself: it holds by
construction and by default, is proven by independent measurement, is honest about its limits, and is
backed by a process for reporting and fixing failures. The track runs alongside the milestones; nothing
ships publicly (M5) before SbD-1 passes.

Already in place: memory-safe Rust with `unsafe_code = "forbid"`; hybrid, sandbox on and network off by
default; commands cannot read sensitive paths (OS-enforced); the agent crate cannot reach the frontier
except through the gate (type-level); project config can only tighten; credentials by environment-variable
name only; 0600 run data; hash-chained audit of every outbound byte; canaries plus an independent leak
proxy; a public threat model (`SECURITY.md`); cargo-deny (advisories, licenses, bans, sources).

**SbD-1 — secure defaults and evidence (before M5)** — done (`aa91182`, §10)
- Refuse plaintext HTTP to a non-loopback local model unless the owner sets
  `local.allow_plaintext = true` (loopback and TLS are always allowed); `duet doctor`-style message on refusal.
- Loosening a privacy setting (registry `confirm` keys, or a value against its tighten direction) through
  `duet config set` needs an explicit `--confirm`, prints the policy diff, and is recorded in an owner
  audit log. Passthrough mode needs an explicit flag and prints a banner (the boundary is off).
- Audit events beyond outbound requests: sandbox denials, `sensitive_data` runs, blocked sends, policy
  and config changes, local-endpoint trust decisions, protected-code decisions.
- Tamper evidence: each run's final chain head is anchored outside the workspace (owner state
  directory); `duet audit verify` checks the anchor, so a rewritten log is detected, not only an edited record.
- `SECURITY.md`: disclosure policy (contact, response times, safe harbour), `security.txt`, commitment
  to publish advisories with CWE root causes; a changelog of the leak classes fixed so far.
- Prompt-injection residual risk documented (the frontier can be steered by hostile public content
  inside the sandbox; what the sandbox and write path still prevent).

**SbD-2 — eliminate classes, verify independently (with M5)**
- Fuzzing and property tests — in place. `fuzz/` (own workspace, excluded from the main one) has
  libFuzzer targets for the SSE parser, chunk assembly with tool-call recovery, vault
  tokenize/detokenize, the copied-span filter and the detectors; `tools/fuzz.sh [seconds]` runs them
  with `cargo +nightly fuzz`, or on stable with the same instrumentation and no sanitizer. Property
  tests (proptest, in the gate with small case counts, `PROPTEST_CASES` to raise): parsers never
  panic and SSE is chunking-independent; vault round trip, idempotence, no value outside tokens,
  aliases never detokenize; copied-span redaction leaves no copied run; detector spans well formed;
  and "no canary survives the gate" over generated `.env`/CSV/log content with canaries in every
  request channel and covered spelling (SECURITY.md, Verification). Bugs found and fixed at the class
  level with regression tests: tokens rewritten by values that spell part of them; values escaped
  inside tool-call arguments (JSON in JSON) missed by the filter and the final check; values inside
  a longer overlapping detection never registered on their own.
- Adversarial review of the boundary before the public benchmark (a red-team pass with fresh canaries,
  encodings and injection), findings fixed at the class level.
- Pre-push hook running `tools/gate.sh`, optional pre-commit hook running `tools/gate.sh --fast`
  (format, license, privacy, provenance) — in place: `tools/install-hooks.sh [--pre-commit]` (no hosted
  CI by operator decision).

**SbD-3 — supply chain and oversight (with M6)**
- Versioned, signed releases; SBOM (CycloneDX); security advisory channel; `duet doctor` flags outdated
  versions.
- Optional approval mode for risky actions (`sensitive_data` commands, writes outside sources/tests).
- Per-run disclosure report: what was withheld, by class (extends the M4 cost ledger).
- Frontier provider data-retention terms documented in the threat model.

**SbD gate (before M5):** every SbD-1 item done with tests; red-team pass (SbD-2) with zero canary
leaks; `SECURITY.md` complete. Any disclosure path found later is fixed at the class level, published as
an advisory, and covered by a regression test before the next release.

### M5 — Public benchmark (days 32–35)

Final gate-size runs on the full S0–L2 suite: `duet-hybrid` vs `duet-passthrough`, plus `claude-code`, `codex`
and `pi-glm` through the proxy (`claude-code` and `codex` on the operator's CLI subscriptions, §3).
The subscription lanes are in `lanes.toml` (`claude-code` via `CLAUDE_CODE_OAUTH_TOKEN` and an isolated
config dir; `codex` via the dedicated `~/.duet-eval/codex` login, upstream the ChatGPT backend);
`duet-eval preflight --lanes ...` checks their prerequisites without model calls, and each lane is
smoke-tested with one run (proxy log shows its model requests) before the runs. Runs record the duet_v2
commit they ran from (`DUET_EVAL_GIT_COMMIT` from the runner, else `git rev-parse HEAD`) and the agent's
version.

Publish `docs/BENCHMARK.md` with `duet-eval report --final <batch...>` (runs paired across batches by task,
seed and lane; a JSON twin is written beside it): per lane and per task hidden pass rate, full success,
judge score, leaks (canaries by kind, exact binomial bound), cost with the paired ratio against
`duet-passthrough` (the privacy premium), wall clock, frontier and local tokens, each with 95% intervals;
the gate verdicts under the current rules; the method (canaries, proxy, judge models, CLI versions and
rubric version, pricing with source and date, statistics); model, build and agent versions; and links to
the raw data with per-run digests. The same raw data regenerates the same bytes.

Every run is judged by both judges (`duet-eval judge <batch>`, Claude CLI and Codex CLI by default; §3,
2026-09-24). The judge score the gates use is the mean of the two; `--final` requires both on every run,
and a run missing one is listed as incompletely judged and left out of the judge gate. The report shows
each judge's mean per lane separately, inter-judge agreement (mean absolute difference and correlation)
and flags lanes judged by their own model family (`claude-code` by the Claude judge, `codex` by the
OpenAI judge).

### M6 — Product hardening

- **TUI (`duet-tui`):** registry-generated screens — Models, Sensitivity, IP levels, Limits, Data,
  Audit, Run; confirmation, policy diff and audit on any loosening; value origins shown; owner-only
  keys never written to project config; snapshot tests per screen. (Run and Audit included; operator kept
  the whole TUI in M6.)
- Setup and presets for Ollama, LM Studio, llama.cpp, vLLM, oMLX, z.ai, Anthropic, OpenAI; no-config
  bootstrap detecting local servers on default ports; `duet doctor` with cache-reuse check.
  *Built (branch `m6doctor`):* local presets (`duet config preset`: Ollama, LM Studio, llama.cpp,
  vLLM, oMLX, mlx_lm.server; audited, `--confirm` for the endpoint change); the loopback-only
  bootstrap in `duet run` (single unambiguous server for one run, else the exact config commands;
  never writes config); `duet doctor` (offline by default, `--online` for listings and the local
  context window, `--json`, exit code = worst result). *Open:* frontier presets (z.ai, Anthropic,
  OpenAI — the latter two need their dialects), the cache-reuse check (needs model calls; `duet
  local-eval` measures it today), an update check (needs a release channel, SbD-3).
- One live smoke test per local backend. *Built:* ignored tests in
  `crates/duet-boundary/tests/backend_smoke.rs`, gated by `DUET_LIVE_<BACKEND>_URL`; not yet run live.
- Large repositories: dogfood tasks X1 and X2 (real open-source repositories with injected
  sensitive assets), repository map, search scaling.

## 6. v1 defects fixed while porting

| Area | v1 location | Fix |
|---|---|---|
| Usage poisoning (`u64::MAX`) | `src/provider/mod.rs` 395, 1522–1528, 5505, 5527, 9423; `src/provider_ledger.rs` 151, 214 | Zero or estimated; separate unknown counter |
| Mid-stream errors not retried | `mod.rs` 7702–7707, 8117–8122, 8344–8349 | Retry as transient |
| Length/filter stops retried | `mod.rs` 7614–7623, 8372–8380, 9196 | Terminal |
| No cache-write/cache pricing | `mod.rs` 10835–10841; `src/benchmark.rs` 79–128 | New fields, price table |
| Prompt-cache busting | `mod.rs` 6277, 921–931, 6983 | Fixed sorted tools; no history rewrite |
| Writable `.git`, no `/run` tmpfs, PATH-resolved sandbox | `src/tools/shell.rs` 697–734, 874–883, 125–131 | Deny, tmpfs, absolute paths |
| Unsandboxed `git status` with fsmonitor | `src/journal.rs` 2001–2008 | Single git helper |
| Reads by path | `src/tools/mod.rs` 2863; `path_guard.rs` 17–40 | Handle-relative reads |
| Project config could set credentials/endpoints | review finding S2 | Owner-only keys |
| 4,096-file cap | review §6 | `git ls-files` |
| `.duet` paths missing from ignore/reset | review §10.4 | Path registry + test |

## 7. Risks

| Risk | Mitigation |
|---|---|
| Digests omit detail the frontier needs | `ask_local` follow-ups; deterministic error-line prepass; `read_raw` for public content; Gate 2 ablations |
| Secret in source escapes path policy | Content detectors on every outbound byte; canaries planted in source |
| Local model copies raw values or is prompt-injected | Extract-only schema; overlap filter; deterministic redaction; injection canaries |
| Cost gate against a cheap GLM baseline | Measure baseline and caching in M0.4; bulky offload, masking, caching, local brief; if still not cheaper, report a measured privacy premium (operator) |
| Local latency | Micro-eval selects a fast model with working prefix cache; wall clock reported |
| Judge noise | Primary quality is deterministic hidden tests; judge only for secondary code quality, two repeats, pinned |
| Tasks too easy or too hard to discriminate | Pilot calibration band (30–90% hidden-test pass for the reference); sealed task versions |
| Protected-code quality | Scoped to marked paths; frontier keeps spec and tests; cost reported |
| Others copy the idea | Edge is measured guarantees and a published benchmark |
| Scope creep | Line budgets, gates, one variable per run |

## 8. Verification

| When | Command / check |
|---|---|
| Every commit | `tools/gate.sh` |
| M0 | `duet-eval selftest`; `duet-eval validate`; `duet-eval check <task>`; `duet-eval run --lanes pi-glm --task S0,S1,S2,M1,M2,M3,L1 --seeds 1-5 --out <batch> && duet-eval report <batch>` |
| M1 | `cargo test -p duet-provider`; live tests |
| M2 | Sandbox escape tests (`.git` write, `/run` socket, detached child); `duet run --mode passthrough --no-privacy --objective-file <task>` reaches `Completed`; resume after kill; Gate 1 report (`duet-eval judge`, `duet-eval report --gate duet-passthrough:pi-glm`) |
| M3 | `duet local-eval`; `duet run --mode hybrid` on the Gate 2 tasks; `duet audit verify <run>`; `leaks.jsonl` empty; Gate 2 report |
| M4 | Gate 3 report |
| M4.5 | L2 runs; IP-canary report |
| SbD | SbD gate: SbD-1 tests, red-team pass, fuzzing (SbD-2) |
| M5 | Lane smoke tests on subscriptions; `duet-eval report --final` (to be added) regenerates `docs/BENCHMARK.md` |
| M6 | TUI snapshot tests; per-backend live smoke tests; X1 and X2 runs |

## 9. Prerequisites

**Operator environment (read from v1 `~/.duet/models.local.toml`, 2026-09-23; values of keys never stored in docs):**

| Role | Server | Endpoint | Model | Auth |
|---|---|---|---|---|
| Frontier | z.ai coding API (OpenAI-compatible Chat Completions) | `https://api.z.ai/api/coding/paas/v4` | `glm-5.3-flash` (switched from `glm-5.3` on 2026-09-23) | bearer, `ZAI_API_KEY` |
| Local | oMLX on a LAN host (OpenAI-compatible) | `http://192.168.50.132:8080/v1` | `omlx-coding` (also `:mechanical`, `:semantic`); 65,536-token context | bearer, `OMLX_API_KEY` |

Consequences for the design:
- The local server is **not loopback**: it is a LAN host over plain HTTP. It must be in the owner
  allowlist for the local role, and since SbD-1 plain HTTP to it is refused unless the owner sets
  `local.allow_plaintext` (the operator opted in; recorded in the config audit log, and every run
  warns). `SECURITY.md` treats that host and network path as trusted. Recommended: an SSH tunnel or
  TLS to it, so sensitive content does not cross the LAN in clear text.
- The local context is 65,536 tokens: the digester chunks handles into ~60K-character pieces (~33K
  tokens of log text) and merges chunk digests; `ask_local` answers from the most relevant chunk.
- The z.ai coding endpoint is a subscription plan; cost gates still use API list prices.

Prerequisites:

- z.ai API key (GLM frontier, `pi-glm` lane).
- The operator's logged-in `claude` CLI (judge). For M5: a `claude setup-token` subscription token
  for the `claude-code` lane and a dedicated ChatGPT-logged-in `CODEX_HOME` for the `codex` lane
  (§3); no Anthropic or OpenAI API keys.
- An OpenAI-compatible local model server (oMLX, LM Studio, llama.cpp, vLLM or Ollama) on loopback
  or an allowlisted host.

## 10. Progress

| Date | Milestone | Status |
|---|---|---|
| 2026-09-23 | M0.1–M0.3 | Done (`e53c700`). Harness `duet-eval` (33 tests); dogfood tasks S0–S2, M1–M3, L1 all pass `duet-eval check`. |
| 2026-09-23 | M0.4 | GLM caching verified; prices verified. Pi+GLM pilot running. Claude Code/Codex lanes and the judge wait for `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`. First measurement: Pi+GLM on S1 passed 9/9 hidden tests and sent all three planted secrets to the provider in 13 of 14 requests. |
| 2026-09-23 | M1 | Chat Completions provider done: stable request prefix, streaming assembly, retries without usage poisoning, mid-stream error retry, terminal length/filter stops, Retry-After, first-byte and idle deadlines, loopback/allowlist enforcement for the local role, text tool-call recovery, context-window probes, built-in prices (27 tests). Live: GLM tool round trip with cache hits; oMLX tool call with prefix reuse (repeat 11.1 s vs 20.8 s cold on a 4K prompt). |
| 2026-09-23 | M2 | Done (`8855ada`, `8fa5976`). Guarded file I/O, sandbox (`.git`/`.duet` read-only at any depth, network off), hardened git helper, settings registry, frontier loop with fixed sorted tools, gate + hash-chained audit, `duet run/resume/audit/config/purge`. Judge runs through the logged-in `claude` CLI. Frontier switched to `glm-5.3-flash` (operator). Runs decided by infrastructure (quota, 4xx on every request) are marked invalid, excluded and retried. |
| 2026-09-23 | M0.4 pilot (Flash) | Pi + glm-5.3-flash, 7 tasks × 5 seeds, all valid. Hidden pass rate 90.7% (full success 66%), mean $0.0105 and 229 s per run. **Pi sent planted canaries to the provider in 30 of 30 runs that had sensitive data** (4,908 occurrences: persons, emails, numbers, phones, secrets, passwords, injection phrases). Calibration: M1–M3 are at 100% on every seed and L1/S2 at 92–94%, so with this frontier they separate lanes only through the judge. The fix is harder tasks (L2, XL), not stricter hidden tests for unstated requirements. S0 fails 2 of 5 by keeping a submodule private that the objective requires public (a genuine miss). |
| 2026-09-23 | M3 (in progress) | Security engine in hybrid mode (`752c1d8`, `5bc9c82`, `332b176`, `4683d5d`): detectors, vault, copied-span filter, handles + local digest, `ask_local`, priming, secret-sink check. Hybrid S2 smoke: 0 leaks, 90% hidden, completed, but $0.054 and 948 s vs Pi's $0.009 (39 frontier turns vs 8–17; 3× the output). A one-word name crossed via a local answer and was invisible to the proxy (hard-coded, not a canary); fixed in the detector and the task. duet sent no `reasoning_effort` while the Pi lane sends `medium`, a confound for Gate 1; fixed (`e3e3ad1`) before Gate 1 started. |
| 2026-09-23 | Gate 1 (Flash) | `duet-passthrough` vs `pi-glm`, S0/S1/S2/M1 × 5, all 40 runs valid, judged by Claude (`claude-opus-5-5`, 2 repeats). **Judge: Δ +0.8/30, lower bound −0.2 → pass** (duet ≥ Pi on S1, S2, M1; −0.6 on S0). Hidden pass rate: 83.5% vs 85.2%, Δ −1.7 pp, lower bound −16.5 pp → not established. The whole gap is S0 (duet 2/5, Pi 3/5), whose runs score 0% or 100% and fail the same way in both lanes (a required-public submodule left private). At that variance the −5 pp margin needs ~430 pairs, so pass rate cannot decide Gate 1 at the planned N; operator decision pending. Cost: $0.0113 vs $0.0099 per run. |
| 2026-09-23 | M3 leak fixes + verification | Hybrid batch 1 (18 runs) leaked in 5 runs; four classes fixed at class level (`2db4cc7`, `1d57c0e`): assistant echo, detector window, value spellings, and derived data from commands, which is now stopped by the OS sandbox (sensitive paths unreadable unless `sensitive_data`, whose output stays local). Local model selected by micro-eval (`dbf7663`): accuracy/evidence/digest/schema 1.00, 0 leaks, prompt cache reuse 92% after one shared schema and content-first prompts. **Verification batch (18 runs, 6 tasks): 0 leaks, 0 sink violations, hidden pass rate 97.9%; judge 17.0–20.2/30.** Gate 2 needs the passthrough baseline on M2/M3/L1 (running). |
| 2026-09-23 | M4 (merged `422c7b8`) | PublicBulky offload (head + outline + `read_raw` ranges), turn-wise masking with stubs, per-run cost ledger by content class read by duet-eval. Turn fixes before it: batched `ask_local`, sensitive paths named in the task (`129e5fe`). Gate 3 not yet measured. |
| 2026-09-23 | M4.5 (merged `8edf98b`) | Interface-only (tree-sitter skeletons for Rust/TS/Python) and Sealed paths, `edit_protected` (local model implements, host runs checks, pass/fail back), protected code in the gate's filters, task L2 (starter 0/14, reference 14/14). IP gate not yet run. |
| 2026-09-23 | Tasks (merged `1372c37`) | L3 parcel-billing (8.4K-line Rust codebase, 25 hidden tests in 7 binaries) and L4 shift-payroll (TypeScript, 44 hidden tests in 7 files); hidden tests may be several commands, so S0 now gives partial credit (1/8 starter, 8/8 reference). Not yet calibrated live. |
| 2026-09-23 | **Gate 2 (Flash): PASS** | `duet-hybrid` (verification batch, build `a5346f4`) vs `duet-passthrough`, S1/S2/M1/M2/M3/L1 × 3 seeds, all runs valid (`results/gate2`). Privacy: 0 leaks and 0 sink violations in 18/18 hybrid runs (passthrough: 3,130 canaries across 18/18). Quality: judge Δ +0.1/30, lower bound −0.8; hidden pass rate 97.9% vs 98.3%, Δ −0.5 pp, lower bound −1.5 pp; behind on 1/6 tasks. Cost (not part of Gate 2): $0.0310 vs $0.0130 per run, wall 496 s vs 239 s — the M4 problem. |
| 2026-09-24 | Gate 3 attempt A: FAIL | Build `aeda66e` (M4 + M4.5 + authored-values fix), hybrid vs passthrough on L3 × 3 and S1/S2/M1 × 3 (`results/gate3a`). 0 leaks. Cost $0.066 vs $0.039 per run; S1 now cheaper than passthrough ($0.0054 vs $0.0058). L3 hybrid: pass 96/76/20% (seed 3 crashed in the copied-span filter, fixed `79fedc3`), $0.10–0.25 vs $0.09–0.15, 67–84 min vs 7–8 min, 81–95 turns vs 36–51 — from 9–11 read_raw after offloaded source files and 21–30 ask_local on a 1,450-line log. Before it: turn fixes cut M1 requests 53→30 (hybrid 2.4×→1.9× passthrough cost); remaining gap traced to 3.7× uncached input from prompt-cache breaks (frontier-authored test data vaulted, fixed `aeda66e`). Attempt B (`9a8026c`): requested files shown whole up to 12K tokens without a local summary; long sensitive texts show repeated line shapes. |
| 2026-09-24 | SbD-1 (merged `aa91182`) | Plaintext non-loopback local model refused unless owner opts in (operator opted in for the LAN host, recorded in the config audit log); loosening via `duet config set` needs `--confirm` and is hash-chain audited; passthrough needs `--no-privacy` + banner; run audit records security events without content; audit heads anchored outside the workspace (`duet audit verify` detects rewrites); SECURITY.md disclosure policy (contact placeholder kept by operator decision), advisories DUET-2026-001…005 with CWE root causes, prompt-injection residual risk. SbD gate still needs SbD-2 red team and fuzzing. |
| 2026-09-24 | Gate 3 attempt C + outcome | Build `4384ca0` (local task brief), `results/gate3c`. 0 leaks. L3 96/96/96%, $0.38/$0.17/$0.20 (mean $0.25 vs attempt B $0.18, passthrough $0.12), 72–91 min; ask_local 10.0 calls/run (B: 5.8). S2 seeds 2–3 invalid: the brief cleaning vaulted a title-case phrase also in the task, and the final check blocked the first send (fail-closed; fixed: title-case runs made only of public words are not names). Outcome: measured privacy premium; brief off by default. Also found in the docs audit and fixed: commands could read `.git` and `.duet/` (DUET-2026-006, `74f17a8`). |
| 2026-09-24 | **IP gate (M4.5): PASS** + L4 calibration | Build `a67ae67`, `results/l2l4`. L2: hybrid 0 leaks in 3/3 runs incl. protected-source canaries, 100% hidden; passthrough 154–355 canaries per run incl. 102 source-canary hits; hybrid $0.022–0.025 vs $0.005–0.012. L4: passthrough 100% on all seeds (too easy for the frontier: outside the 30–90% band); hybrid 100/63.6/100%, 0 leaks vs 2,603–6,446 per run, $0.12–0.25 vs $0.04–0.10, 54 vs 29 turns, 12 ask_local calls per run. One L4 hybrid run first failed grading on a full disk (ENOSPC), was set aside and re-run; host failures are now invalid automatically (`24d921d`). Parallel round merged: M6 non-TUI (`68fb66f`), M5 prep (`29d6209`), dual judging (`a84cbf4`), SbD-2 verification (`28a9383`; advisories DUET-2026-007/008). Dogfood runs now live on EXT_DISK (`results` symlink). |

Scope changes, with reasons:
- **Anthropic Messages and Responses dialects move to M6.** The frontier (z.ai GLM) and the local
  server (oMLX) both speak Chat Completions, so no gate depends on the other dialects.
- **The local-model micro-evaluation moves to M3.** It measures the digest and answer roles, whose
  prompts and schemas are written in M3; it runs before Gate 2.
