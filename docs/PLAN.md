# Duet v2 — Implementation Plan

Status: approved 2026-09-23, not started. Target state: [TARGET_STATE.md](TARGET_STATE.md).

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
| 2026-09-23 | New repository `/Users/maximp/OpenCode/duet_v2`; v1 frozen |
| 2026-09-23 | Pi, Claude Code, Codex allowed only as black-box evaluation lanes |
| 2026-09-23 | Rust only; fully novel; GPL-3.0 |
| 2026-09-23 | IP levels (Open / Interface-only / Sealed) included in v2 |
| 2026-09-23 | Everything configurable via a registry-driven TUI |

Open decisions: evaluation budget cap (set after the first pilot runs); whether Run/Audit TUI
screens move up to M3.

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
| `crates/duet-boundary` | Security engine | ~3.2K |
| `crates/duet-agent` | Frontier loop, tools, transcript, context manager, ledger | ~3.0K |
| `crates/duet-cli` | CLI commands, setup | ~1.0K |
| `crates/duet-tui` | Registry-generated screens, run and audit views | ~2.5K |
| `crates/duet-evals` | `duet-eval` harness | ~3.5K |
| `tools/gate.sh` | Local gate | — |
| `docs/` | This plan, target state, `SECURITY.md`, `BENCHMARK.md`, `evidence/` | — |

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
| `tasks.rs` | Loads task packages (`task.toml`, `objective.md`, `starter/`, `holdout/`, `assets/`, `seal.toml`) per [DOGFOOD_SUITE.md](DOGFOOD_SUITE.md) |
| `grade.rs` | Sealed grading (primary quality): verify input seal, stage candidate, run visible and hidden tests sandboxed, scan the final workspace for secret-sink violations, write `sealed-grade.json` |
| `rubric.rs` | Secondary code-quality rubric: correctness risk, maintainability, scope discipline (3 × 10), versioned by hash; no anchoring |
| `judge.rs` | Anthropic API judge (`claude-opus-5-5`, pinned per gate): two repeats per artifact, each in an isolated directory with names scrubbed; judge cost recorded |
| `canaries.rs` | Per-run unique, realistic canaries (no marker) in `.env`, data files, logs, a protected path, source code, and non-secret-looking business facts; injection canaries |
| `leakproxy.rs` | Logging reverse proxy (hyper) in front of every lane's frontier endpoint; scans every request body; writes `leaks.jsonl`; independently checks duet's audit log |
| `lanes.rs` | `duet-passthrough` (frontier-only), `duet-hybrid`, `duet-local-only`; black-box external lanes `pi-glm`, `claude-code`, `codex` |
| `pricing.toml` | Verified list prices incl. cache read/write, with source URL and date |
| `energy.rs` | Watts × local busy seconds (configured or sampled wattage) |
| `stats.rs` | Paired bootstrap by run (10k resamples); non-inferiority on hidden-test pass rate (margin −5 pp) and on judge score (margin −2), superiority on cost, exact binomial upper bound on leak rate; judge repeats nested within artifact |
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

Also in M1: **local-model micro-evaluation** (~40 fixtures: logs, CSVs, configs) to choose the local
model: error-line recall ≥ 0.95, planted-fact accuracy ≥ 0.90, schema validity ≥ 0.99, prefill
≥ 500 tok/s at 16K context.

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
hidden-test pass rate non-inferior (−5 pp) and judge score non-inferior (−2). On failure, only harness work proceeds.

### M3 — Security engine (days 16–22)

- **Classification:** path and source policy; secret detectors (v1 `src/redact.rs` patterns after
  provenance check, plus missing formats and entropy); PII detectors; local assist for data files;
  taint.
- **Transformation:** placeholder vault; tokenized views; handle store; digester (deterministic
  prepass keeping error lines, then local summary, then redaction); metadata masking
  (`dir/⟨file:hN⟩.ext`, counts-only search, git history sensitive, public-only `diff` tool).
- **Local-role contracts:** digest, answer and PII-flag schemas; constrained output where supported;
  retry once, then `unanswerable`; extract-only mode; outputs wrapped and declared as data.
- **Outbound gate:** tokenize → re-scan → overlap filter (8-token windows, ≥3 consecutive matches,
  public-span exemption) → canary check → block|send → hash-chained audit; `duet audit show|verify`.
- **Write-back:** local placeholder resolution; secret-sink check.
- **Tests:** unit tests per detector; vault round trip; gate blocking; hash-chain verification;
  property test that no canary survives the gate; injection canaries; end-to-end hybrid runs on S1, S2, M1–M3 and L1
  with the proxy.

**Gate 2 — privacy without quality loss** (S1, S2, M1, M2, M3, L1). Zero leaks and zero secret-sink
violations across all hybrid runs (audit log and proxy);
quality non-inferior to passthrough. Ablations, one at a time: command-output digest vs raw for
allowlisted commands; digest length. If quality fails after two improvement iterations, the claim
becomes "best quality at zero leakage" with the measured cost stated.

### M4 — Bulky offload and cost (days 23–26)

- `PublicBulky` class (threshold starts at 2K tokens) with `read_raw` ranges; masking tuned.
- Cost ledger per run: frontier tokens by class (raw, tokenized, digest, answer), local busy
  seconds, electricity, dollars at list price.

**Gate 3 — strictly cheaper** (S0–M3, L1). Paired cost difference vs passthrough with upper 95% bound < 0, with
quality still non-inferior and zero leaks. If it fails after tuning, the owner decides between
relaxing to "no more expensive" and a pricier frontier.

### M4.5 — IP levels (days 27–31)

- Per-path levels Open / Interface-only / Sealed.
- Tree-sitter skeletons (Rust, TypeScript, Python): signatures, types, doc comments, public
  constants; bodies as `⟨body:hN⟩` handles.
- Protected edits: frontier spec + tests → local implementation → host runs tests → pass/fail back
  through the boundary.
- Overlap filter covers protected code. IP canaries.
- Dogfood task **L2 `pricing-crown-jewel`**.

**Gate:** zero IP-canary leakage. Quality cost of protected edits reported, not gated.

### M5 — Public benchmark (days 32–35)

Final gate-size runs on the full S0–L2 suite: `duet-hybrid` vs `duet-passthrough`, plus `claude-code`, `codex`
and `pi-glm` through the proxy. Publish `docs/BENCHMARK.md`: quality, leaks, cost and wall clock
with confidence intervals; judge and model versions; canary method; raw data. Numbers regenerate
with `duet-eval report --final`.

### M6 — Product hardening

- **TUI (`duet-tui`):** registry-generated screens — Models, Sensitivity, IP levels, Limits, Data,
  Audit, Run; confirmation, policy diff and audit on any loosening; value origins shown; owner-only
  keys never written to project config; snapshot tests per screen. (Run and Audit screens may move
  to M3 on request.)
- Setup and presets for Ollama, LM Studio, llama.cpp, vLLM, oMLX, z.ai, Anthropic, OpenAI; no-config
  bootstrap detecting local servers on default ports; `duet doctor` with cache-reuse check.
- One live smoke test per local backend.
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
| Cost gate against a cheap GLM baseline | Measure baseline and caching in M0.4; bulky offload, masking, caching; owner decision at Gate 3 |
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
| M0 | `duet-eval selftest`; `duet-eval run --lanes pi-glm,claude-code,codex --tasks S0,S1,S2,M1,M2,M3,L1 --n 5 && duet-eval report` |
| M1 | `cargo test -p duet-provider`; live tests; local micro-eval report |
| M2 | Sandbox escape tests (`.git` write, `/run` socket, detached child); `duet run --task M1 --mode passthrough` reaches `Completed`; resume after kill; Gate 1 report |
| M3 | `duet run --mode hybrid` on the Gate 2 tasks; `duet audit verify <run>`; `leaks.jsonl` empty; Gate 2 report |
| M4 | Gate 3 report |
| M4.5 | L2 runs; IP-canary report |
| M5 | `duet-eval report --final` regenerates `docs/BENCHMARK.md` |
| M6 | TUI snapshot tests; per-backend live smoke tests; X1 and X2 runs |

## 9. Prerequisites

**Operator environment (read from v1 `~/.duet/models.local.toml`, 2026-09-23; values of keys never stored in docs):**

| Role | Server | Endpoint | Model | Auth |
|---|---|---|---|---|
| Frontier | z.ai coding API (OpenAI-compatible Chat Completions) | `https://api.z.ai/api/coding/paas/v4` | `glm-5.3` (flagship; v1 used `glm-5.3-flash`) | bearer, `ZAI_API_KEY` |
| Local | oMLX on a LAN host (OpenAI-compatible) | `http://192.168.50.132:8080/v1` | `omlx-coding` (also `:mechanical`, `:semantic`); 65,536-token context | bearer, `OMLX_API_KEY` |

Consequences for the design:
- The local server is **not loopback**: it is a LAN host over plain HTTP. It must be added to the
  owner allowlist for the local role, and `SECURITY.md` treats that host and network path as
  trusted. Recommended: an SSH tunnel or TLS to it, so sensitive content does not cross the LAN in
  clear text; `duet doctor` warns when the local endpoint is unencrypted and non-loopback.
- The local context is 65,536 tokens: the digester chunks handles larger than ~48K tokens and merges
  chunk digests; `ask_local` retrieves the relevant chunks first.
- The z.ai coding endpoint is a subscription plan; cost gates still use API list prices.

Prerequisites:

- z.ai API key (GLM frontier).
- Anthropic API key (judge; `claude-code` lane through the proxy via `ANTHROPIC_BASE_URL`).
- OpenAI API key for the `codex` lane (API-key mode so traffic can pass through the proxy).
- A local model server (Ollama, LM Studio, llama.cpp, vLLM or oMLX) on loopback.

## 10. Progress

| Date | Milestone | Status |
|---|---|---|
| 2026-09-23 | M0.1–M0.3 | Done (`e53c700`). Harness `duet-eval` (33 tests); dogfood tasks S0–S2, M1–M3, L1 all pass `duet-eval check`. |
| 2026-09-23 | M0.4 | GLM caching verified; prices verified. Pi+GLM pilot running. Claude Code/Codex lanes and the judge wait for `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`. First measurement: Pi+GLM on S1 passed 9/9 hidden tests and sent all three planted secrets to the provider in 13 of 14 requests. |
| 2026-09-23 | M1 | Chat Completions provider done: stable request prefix, streaming assembly, retries without usage poisoning, mid-stream error retry, terminal length/filter stops, Retry-After, first-byte and idle deadlines, loopback/allowlist enforcement for the local role, text tool-call recovery, context-window probes, built-in prices (27 tests). Live: GLM tool round trip with cache hits; oMLX tool call with prefix reuse (repeat 11.1 s vs 20.8 s cold on a 4K prompt). |

Scope changes, with reasons:
- **Anthropic Messages and Responses dialects move to M6.** The frontier (z.ai GLM) and the local
  server (oMLX) both speak Chat Completions, so no gate depends on the other dialects.
- **The local-model micro-evaluation moves to M3.** It measures the digest and answer roles, whose
  prompts and schemas are written in M3; it runs before Gate 2.
