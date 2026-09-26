# Duet v2 — Implementation Plan

Status: approved 2026-09-23; in progress (M0–M4.5 and SbD-1 done; M5 and M6 under way; acceptance
audit in [ACCEPTANCE.md](ACCEPTANCE.md)). Progress: §10. Target state: [TARGET_STATE.md](TARGET_STATE.md).

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
3. **Novelty and provenance.** No code, formats, prompts, tool schemas or design documents from any
   coding agent. Each v1 file is checked before porting (agent-name/format grep, git history).
   Excluded: v1's patch envelope (`src/tools/patch.rs`), the Codex CLI transport and presets, and v1
   documents comparing other agents. Public API wire formats are protocols and are allowed.
   `tools/gate.sh` fails on agent names or formats in `crates/`, except the eval lane adapters.
4. **Rust only.** No Python anywhere, including the evaluation harness.
5. **License GPL-3.0**, SPDX headers, dependency licenses checked by cargo-deny.
6. **No hosted CI.** `tools/gate.sh` is the gate: fmt, clippy `-D warnings`, `cargo test`,
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
| 2026-09-24 | Red-team pass parked until last (operator): it is the final step before the benchmark is published; it no longer blocks other work (M5 runs may proceed, publication of `docs/BENCHMARK.md` waits for it) |
| 2026-09-24 | Line budgets removed (operator): no per-crate line limits; the 2026-09-23 duet-boundary budget decision is superseded |
| 2026-09-24 | Releases and SSH signing wait until the application is verified to work and meet its requirements (operator): acceptance first, then SbD-3 release signing is used |
| 2026-09-24 | Live backend acceptance = the configured remote oMLX server (operator): it is verified live (doctor --online, micro-eval, all gate runs, hands-on acceptance); Ollama, LM Studio, llama.cpp, vLLM and mlx_lm.server stay verified against mock servers only, stated as such |
| 2026-09-24 | Future TUI (operator preference): a dual-panel Run view — main panel with the live run (turns, tool calls, results, withheld-content events), side panel listing the files the run changed (+/- line counts) with a scrollable per-file diff (line numbers, added/removed highlighting) that follows the run as it edits. Recorded as a functional requirement; the layout and visuals are designed independently (novelty rule: no design references from other coding agents) |
| 2026-09-25 | Detection rules stay duet's own engine, fed by a maintained third-party rule set as data (operator): import the gitleaks rule set (MIT; a secret scanner, not a coding agent, so the novelty rule does not exclude it) into duet's detector, pinned and updatable; add a detection corpus that measures recall and false positives in the gate; widen personal-data formats beyond US-only; optional local-model pass for names and addresses in free text. See SbD-2 "Detection coverage" |
| 2026-09-26 | Local security review is part of the product (operator): one SAST engine with two entry points — an **auditor** of duet's own changes at `finish` (first) and a **scanner** of the whole repository (`duet scan`, second). Three layers: deterministic rules find candidate paths and known patterns; the local model judges every candidate and is the only reviewer of protected code and privacy flows; an optional fresh-context frontier second opinion for high-severity or uncertain findings on open code. Only rule-confirmed high-severity paths can block. The local layer's value is measured on public vulnerable-code benchmarks before it is trusted. See M5.3 |
| 2026-09-26 | Web search is executed by the host itself (operator): duet's own code queries sources with official open APIs from the operator's machine (no search-provider API by default, no HTML scraping of general search engines); z.ai search and SearXNG/Brave remain explicit owner choices |
| 2026-09-26 | Cost with security (operator: "yes"): the local model returns to cutting frontier cost, now aimed at the measured driver — frontier turns × context size (XL: 159 turns, ~120K tokens resent per turn, input ≈ 90% of frontier cost; privacy mode's `ask_local` round trips are 23% of S–L tool calls). Rule: offload only what removes frontier turns or shrinks context; never add turns; the local model never decides; everything it produces passes the boundary. See M5.2 |
| 2026-09-26 | Everyday use at parity with popular agents (operator: "go" after a full-stack app test): registry-only network for ordinary commands **on by default** (they cannot read sensitive files, protected code or run state, so what they can send is public code the frontier already sees; `sensitive_data` commands and checks of protected code never get network); host-side egress proxy with a host allowlist, audited; detector false positives on build artifacts and paths fixed; work without git; `DUET.md` project instructions (untrusted repository text, can never loosen policy); chat line editing, history and streamed replies; commits offered in interactive sessions (asked inline). See M5.1b |
| 2026-09-26 | System prompts matched to the request (operator, after a live one-shot build of a named game: the model never looked up the game, wrote it in one 9K-token `write_file`, checked only that its script parsed, finished in 6 turns, and quietly changed the brief to "an original game inspired by" it; the prompt was tuned for benchmark bug fixes): fixes keep the focused behaviour; building something new means understanding the brief first (looking up what it names with the web tools, named only when offered), keeping to it (assumptions stated in a run, real decisions asked in a session), planning, building in checkable steps and running what was built before finishing; plus tidy work. The one-shot prompt is pinned by a digest test. **Any change to the system prompts changes every run's behaviour and request prefix, so Gate 2 quality and cost are re-measured on the new prompt before M5** |
| 2026-09-25 | Practical toolset before M5 (operator: "secure but also very practical"; missing tools block developer experience): web search/fetch, MCP servers (the only plugin mechanism), sub-agents, language-server tools, image input, git history and commits, plus steering in sessions. Each is a new input or output channel and gets the same treatment as the existing tools: results through the boundary by trust class, outbound text checked, spawned processes sandboxed, every use audited, side effects behind the approval mode. See M5.1 |
| 2026-09-25 | Interactive sessions before M5 (operator): duet must support coding in a conversation, not only one-shot tasks — `duet chat` and an input panel in the TUI Run view; follow-ups and corrections keep the context; duet may ask clarifying questions; review between steps with the diff panel. Same privacy engine, sandbox, audit and approval rules; user messages are sanitized like task text. Designed independently (novelty rule) |

Open decisions: evaluation budget cap (set after the first pilot runs).

Resolved 2026-09-23: GLM prompt caching works on the z.ai coding endpoint. `glm-5.3` with a
28,546-token prompt sent twice reported `cached_tokens` 0 then 28,544 (cache read $0.26/M vs input
$1.40/M). No fallback frontier is needed.

## 4. Repository layout

| Path | Contents |
|---|---|
| `crates/duet-provider` | Model APIs, streaming, retry, credentials, usage, pricing |
| `crates/duet-fs` | Guarded file access, atomic writes, lock, spill, `.duet` registry |
| `crates/duet-sandbox` | Seatbelt/bwrap, env allowlist, process-tree control |
| `crates/duet-git` | Private store, single git helper |
| `crates/duet-config` | Settings registry, scopes, tighten-only rule, `duet config` |
| `crates/duet-boundary` | Security engine (incl. bulky offload and IP levels) |
| `crates/duet-agent` | Frontier loop, tools, transcript, context manager, ledger |
| `crates/duet-cli` | CLI commands, setup |
| `crates/duet-tui` | Registry-generated screens, run and audit views |
| `crates/duet-evals` | `duet-eval` harness |
| `crates/duet-release` | Release tooling: offline SBOM generator (`duet-sbom`) |
| `tools/gate.sh` | Local gate |
| `docs/` | This plan, target state, dogfood suite, `security.txt`, `evidence/`; `BENCHMARK.md` (M5). `ARCHITECTURE.md` and `SECURITY.md` are at the root |


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
| `leakproxy.rs` | Logging reverse proxy (hyper) in front of every lane's frontier endpoint; scans every request body and every client WebSocket message (`wsframe.rs` reads RFC 6455 frames); writes `leaks.jsonl` (independent of Duet's own audit log) |
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
| XL (2–4 h) | X1 `sql-gateway` (sqlparser 0.47.0, Rust), X2 `partner-exports` (JSONata 2.0.6, JavaScript), built for M5 | real open-source repositories with injected sensitive assets |

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
interval instead of a pass/fail gate (the batches so far measured about 1.5–2.4×, depending on the
tasks); quality non-inferiority and zero leaks remain required.

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
- Detection coverage (own engine, maintained rules as data) — in place (SECURITY.md, Detection):
  gitleaks v8.30.1 vendored, 221 rules in use, 0 not compiled, 1 path-only; corpus recall 228/228
  imported-rule positives and 30/30 hand-written, 0 of 251 positives reach the frontier end to end;
  false positives 53 of 683 hard-negative lines (80 before; the imported rules add none); 10 MB of
  mixed log/code at 34–43 MB/s as one text and 49–50 MB/s in 8 KB pieces (before 50–64 and 58–66),
  the imported rules alone 85–153 MB/s with 12 of 221 past the keyword prefilter.
  - Rule import: the gitleaks rule set (MIT, pinned release, license notice kept) vendored as a data
    file and compiled into duet's detector (regex, per-rule keywords as an aho-corasick prefilter,
    entropy thresholds, per-rule allowlists; rules whose syntax the Rust regex engine rejects are
    reported, not silently dropped). `tools/update-rules.sh <version>` fetches, checks the hash and
    shows the rule diff. Duet's own rules stay and take precedence; secret kinds keep their rule id
    for the disclosure report.
  - Detection corpus in the repo: synthetic positives for every rule (never real credentials) and
    hard negatives (hashes, UUIDs, lockfiles, base64 of public data, identifiers, test fixtures).
    The gate requires every positive detected and fails on a false-positive regression; recall and
    the false-positive rate are reported.
  - Personal data beyond US formats: international phone numbers, IBAN with its mod-97 check,
    national IDs with checksums for the main EU/UK formats, IPv6, postal addresses in labelled fields.
  - Optional local-model pass (`sensitivity.local_pii_pass`, off by default: it costs local time) for
    names and addresses in free text that no pattern can find.
  - Scanning throughput measured on large command output so the added rules stay cheap.
- Adversarial review of the boundary before the public benchmark (a red-team pass with fresh canaries,
  encodings and injection), findings fixed at the class level.
- Pre-push hook running `tools/gate.sh`, optional pre-commit hook running `tools/gate.sh --fast`
  (format, license, privacy, provenance) — in place: `tools/install-hooks.sh [--pre-commit]` (no hosted
  CI by operator decision).

**SbD-3 — supply chain and oversight (with M6)**
- Versioned, signed releases; SBOM (CycloneDX); security advisory channel; `duet doctor` flags outdated
  versions. Signed releases and SBOM — done: `tools/release.sh <version> --key <ssh key>` (gate,
  `--locked` release build, SBOM, `SHA256SUMS`, detached `ssh-keygen -Y` signature with the operator's
  key, never generated or stored in the repo), `tools/verify-release.sh`, `tools/sbom.sh` (own
  generator from `cargo metadata`, offline, `crates/duet-release`); `duet doctor` warns without
  release keys (`allowed_signers`). Open: a published release channel (then the outdated-version check)
  and an advisory feed beyond SECURITY.md.
- Optional approval mode for risky actions (`sensitive_data` commands, writes outside sources/tests) —
  done: `oversight.approve` (owner-only; `off`/`risky`/`all`), y/N on the terminal, refusals are tool
  errors, decisions audited without content, fail closed without a terminal, eval lanes keep it off;
  `duet doctor` shows the mode (SECURITY.md, Oversight).
- Per-run disclosure report: what was withheld, by class (extends the M4 cost ledger) — done:
  `duet audit disclosure <run>` and `disclosure` in `summary.json`, from the audit log and ledger,
  counts and kinds only (SECURITY.md, Disclosure report).
- Frontier provider data-retention terms documented in the threat model — done: SECURITY.md, Trust
  assumptions, "Frontier provider data handling" (Z.ai terms read 2026-09-25, with what they do not
  guarantee).

**SbD gate (before M5):** every SbD-1 item done with tests; red-team pass (SbD-2) with zero canary
leaks; `SECURITY.md` complete. Any disclosure path found later is fixed at the class level, published as
an advisory, and covered by a regression test before the next release.

### M5.0 — Interactive sessions (before the benchmark)

- `duet chat` (terminal) and a conversation input in the TUI Run view (beside the changed-files/diff
  panel): the operator gives a task, duet works, the operator replies with corrections or the next
  task in the same session; the conversation and its context persist (append-only transcript), and a
  session resumes after exit or interrupt.
- duet may ask the operator a clarifying question (a tool that ends the turn and waits for input);
  the operator can stop, redirect or accept between steps.
- Every operator message passes the same boundary as task text (sanitized, audited); sandbox,
  approval mode, budgets and terminal states apply per session and per turn.
- Tests with a scripted frontier; a live hands-on acceptance on a fresh project.
- Steering: a message typed while a turn runs is queued and delivered at the next tool-result
  boundary (never killing a running command); distinct from stop (end the turn after the current
  step) and interrupt (kill now). Sanitized, audited, persisted in place for resume.

*Built (branch `chat`):* `duet chat` and sessions in the TUI Run view. A session is a run whose
conversation continues across operator turns: each message is a turn on the same loop, tools and
boundary, ending with a reply (`reply`, or a message without tool calls), a clarifying question
(`ask_operator`), a finished task (`finish` with the checks) or a stop (failure, per-turn or
session budget `session.frontier_usd` / `session.wall_clock_minutes`, `/stop` after the current
step, Ctrl-C at once); the session stays open until the operator closes it and resumes from its
transcript (`duet chat --resume`). Messages typed while duet works steer the turn: delivered
together at the loop's safe point (after the step's results, before the next request) and replayed
in place on resume. Operator messages are sanitized like task text (the sensitive-path note once
per session) and audited (`operator_message`). `/status`, `/diff`, `/undo` (the last turn's
journaled writes). TUI: `n` session, `o` one-shot run, input box under the conversation, `r`
resume, `s` / `x` stop. Tests with scripted frontiers in process and through the binary.
*Open:* the live hands-on acceptance on a fresh project.

### M5.1 — Practical toolset (before the benchmark)

Every new tool is a new channel, so each follows one contract (SbD):

- **Inbound**: results reach the frontier only through `Presenter::present` with a new `Source`
  whose trust class decides the view — *workspace* (classified by path exactly like `read_file`:
  git history, language-server results), *public-untrusted* (web: scanned, bulky offload, framed as
  data), *per-server* (MCP: `public` or `sensitive`; sensitive results stay local as handles).
  Hidden and IP-protected paths stay hidden in every channel.
- **Outbound**: any text the frontier sends to a third party (search query, URL, MCP arguments) is
  checked first: placeholders are never resolved for a non-local destination and known sensitive
  values refuse the call (fail-closed, audited).
- **Processes**: every spawned server (MCP stdio, language servers) runs in the OS sandbox with the
  same hidden paths as commands, a cleared environment (named variables only) and no network unless
  configured.
- **Side effects** (git commit, MCP tools not declared read-only) follow `oversight.approve`.
- **Audit**: every call is an audit event (channel, target, class, outcome).
- **Stable prefix**: the tool set is fixed at run start and sorted.

| Capability | Tools | Key rules |
|---|---|---|
| Web | `web_search`, `web_fetch` | Host-side HTTP (commands keep no network). GET only; http/https; private, loopback, link-local and metadata addresses refused (checked after DNS and on every redirect) unless allowlisted; size cap; HTML to text. Search works without setup (`web.search.backend = "auto"`): Z.ai when it is the frontier (the coding plan's search server, or the per-search Web Search API), else SearXNG (self-hosted; `duet config preset searxng`), else Brave (`BRAVE_API_KEY`), else Wikipedia (no key); fetch works without a backend. On by default; `web.enabled = false` turns it off |
| MCP | `mcp__<server>__<tool>` | Own client (stdio and streamable HTTP, JSON-RPC 2.0). `[mcp.servers.<name>]`: command/args or url, `env` names, `trust`, `network`, `approve`. Server tool descriptions are untrusted text: scanned and length-capped. Placeholders resolved only for `trust = "sensitive"` stdio servers |
| Language servers | `code_nav` (definition, references, hover, symbols, diagnostics), `rename` | Own LSP client; servers autodetected on PATH or configured; started lazily, sandboxed, protected source readable to them like checks. Results filtered by `path_visible`/IP level; snippets presented as their file's class. Diagnostics appended to edit results |
| Git | `git_log`, `git_show`, `git_blame`, `git_status`, `git_commit` | Through the hardened `duet-git` runner (no hooks, no global config). History is presented by path class (sensitive paths stay sensitive in every revision; historic diffs are scanned). Commits: only paths the session wrote, never sensitive or derived files, no placeholders in the message, operator identity, no push. `.git` stays hidden from commands |
| Sub-agents | `delegate` | A child loop with a fresh context over the same engine (same vault, policy, audit chain); read-only children run in parallel, writing children one at a time and only on the paths given; depth 1; the parent's budget is shared; results are the child's summary and diff |
| Images | attachments, `read_file` on images | Images cannot be text-scanned. Hybrid: described by the local model (needs a vision-capable local model) unless the operator marks an image public or it is a public workspace path and `images.to_frontier` allows it. Pass-through: sent when the frontier supports vision |

Delivery: web, MCP, language servers and git in parallel; sub-agents and images after M5.0 merges
(they touch the run loop and the provider types). Each capability: tests with a scripted frontier
and mock servers, a threat entry in `SECURITY.md`, README usage, and a live check where a server or
backend is available. Follow-up: an egress proxy so commands can reach package registries only.

*Built (branch `subagents`):* `delegate {task, mode, paths?, budget?}` in runs and sessions
(`subagents.enabled`, `max_parallel`, `max_usd`, `max_minutes`, `model`). A sub-agent is
`run::work` one level down with its own fixed prompt and tool allowlist per mode, over the same
engine, gate, audit log and sandbox; the model is a `Driver` parameter (the run's frontier or
`subagents.model` behind its own gate on the same audit chain), so a local-model driver (M5.2) can
plug in. Read sub-agents run in parallel with read-only commands (sandbox `read_only`); a write
sub-agent writes through the journal confined to its globs; depth 1; budgets shared with the
parent; transcript entries nested under its id; `subagent_start`/`subagent_end` audit events; a
sub-agent whose result was never recorded is ended and rolled back on resume. Scripted-frontier
tests (hybrid canaries, confinement, depth, parallelism, budgets, interrupt, stop, crash resume,
undo, separate model). Live check (2026-09-25, `glm-5.3-flash`): in pass-through two read
sub-agents ran in parallel and a write sub-agent limited to `src/export/**` fixed a bug (3
sub-agents, 18 requests, $0.0044 of the run's $0.0060); in hybrid (local oMLX) two parallel read
sub-agents reported on a sensitive CSV through handles and `ask_local`, with no planted value in
the audit log and the chain intact.

### M5.1b — Everyday use (before the benchmark)

A live test (2026-09-26) had duet build a full-stack TypeScript app from an empty repository
(Express + SQLite + React/Vite, 27 tests, build, served on :3000; 8.6 min, $0.037) — but only
with `sandbox.network = true`. Gaps found and fixed here:

- **Registry-only network** (`sandbox.network = "registries"`, the new default; `"off"` and
  `"all"` remain): commands reach an egress proxy on the host that allows only configured package
  registry hosts (crates.io, npm, PyPI, Go proxy, Maven Central, GitHub release downloads by
  default; `sandbox.registries` adds or removes), refuses everything else, and audits every
  connection (host, bytes). `sensitive_data` commands never get network in any mode.
- **Detector false positives** on hashed build artifacts (`index-DVuHW4gw.js`), absolute paths in
  stack traces, and similar identifiers: they hid a debugging-relevant path from the frontier.
  Fixed at the class level with corpus negatives.
- **No git required**: file listing and search fall back to a walk that honours `.gitignore`.
- **Project instructions**: `DUET.md` at the repository root (and `~/.config/duet/DUET.md` for the
  owner) is given to the frontier at the start of a run or session. Repository text is untrusted:
  it passes the boundary like any public content and cannot change policy.
- **Chat**: line editing, history (from the session's own transcript, no new store), replies
  streamed as they arrive.
- **Commits in sessions**: with `git.commit = "ask"`, an interactive `duet chat` asks inline even
  when `oversight.approve` is off; non-interactive runs keep today's rule.
- **Tidy work**: the system prompt asks the agent to remove scratch files it created and to ignore
  runtime data (databases, logs) it creates.

Acceptance: the full-app test repeated with default settings in both modes, and a variant whose
repository holds sensitive data (an `.env` with keys, a customer CSV the app must import).

*Built (branch `everyday`):* no git required (`Git::list_files` falls back to a walk honouring
`.gitignore`/`.ignore`; `diff` and `/diff` compare the files written with their first content;
priming, undo, resume and doctor work; a failed `git` command gets a hint); `DUET.md` (repository
and owner) at the start of runs, sessions and sub-agents, presented or sanitized, framed, capped at
16 KiB, audited (`instructions`), named by `duet doctor`; commits asked inline in an interactive
`duet chat`; system prompts matched to the request, with tidy work (§3). Found on the way:
`diff` in a repository showed a tracked sensitive file's changes as public text (DUET-2026-018,
fixed).

*Built (branch `chatux`):* the chat part. `duet chat` on a terminal gets a console of duet's own
(crossterm, already in the lock; rustyline and reedline draw only while a line is read and print
concurrent output as whole lines, and reedline's in-place repaint needs Rust 1.95): a line editor
(word/line movement, cut and put back, Alt-Enter / Shift-Enter / Ctrl-J new lines, the trailing
`\` kept), history from the session's own transcript (no new store), Ctrl-R search, Tab completion
of commands and `/image` paths; the frontier's text streamed as it arrives (a read-only tap on the
provider stream, set per task by `duet_boundary::live::observe`; placeholders held until complete
and restored locally; the `reply` / `ask_operator` text read out of the call's JSON as it streams)
and rendered as light Markdown wrapped to the terminal, colour unless `NO_COLOR`; a status line
while duet works (time, turn cost, what runs now) in a live region outside the scrollback; typing
still steers; Ctrl-C read as a key and given the same rules; Ctrl-Z suspends. Everything from
outside duet is stripped of escape sequences before it reaches the terminal. Piped output is
unchanged line for line. Also `duet run` progress on standard error (live on a terminal, compact
lines with placeholders kept and a heartbeat otherwise; `--quiet`), so a long generation never
looks hung; standard output keeps only the summary. Checked by hand in tmux (streaming, steering,
`/stop`, Ctrl-C, approval of a commit, history after resume, resize, Ctrl-Z/`fg`, `NO_COLOR`, the
terminal restored on exit).

*Built (branch `fps`):* detector false positives. All six public values of the live run came from
the entropy detector; long tokens are now judged by their parts (names, ids, content hashes before
a build-artifact extension, a Next.js build id; a random long part in a path or URL is still
withheld), and ISBNs are not card numbers (a calibration run's citation). Corpus: five files of
real toolchain output (vite, webpack, next; Node, Python, Rust, Java traces; npm, pip, cargo;
vitest, jest, pytest; docker), false positives 148 → 37 of 1,513 lines, the toolchain lines
91 → 0, and every toolchain output through the hybrid engine without a placeholder; recall
held (228 of 228, 34 of 34, 0 of 255 end to end). Found on the way: the entropy detector's
`__` and camel-case exemptions let about 1% of random URL-safe keys through (DUET-2026-021, fixed).

*Built (branch `egress`):* registry-only network, the default. `sandbox.network` = `off`,
`registries` or `all` (the old booleans read as `all` / `off` with a note; loosening confirmed, a
project only tightens); `sandbox.registries` owner-only, adding a host confirmed. A host-side
proxy (`crates/duet-egress`, own code): `CONNECT` and plain-HTTP `GET`/`HEAD`, only listed names
resolved, every address checked with the web tools' classes, a tunnel's TLS server name must be
its host, one `egress` audit event per connection (host, port, bytes; never a path). One route per
command: Seatbelt lets a command reach the proxy's loopback port and the development ports free on
the host (a positive list: Seatbelt ignores a port `deny` after a wildcard `allow` for some ports,
found here); bubblewrap keeps `--unshare-net` and a helper inside (`duet __sandbox-bridge`) hands
each connection to the host over `SCM_RIGHTS`, so the `AF_UNIX` seccomp filter stays. Checks that
can read protected source and `sensitive_data` commands never get network; credential stores in
the home directory are unreadable to every command; per-run package caches (cargo seeded with
links to the operator's crates). github.com is not a default (it takes pushes); its download hosts
are. Live check (2026-09-26, macOS, default settings, through the sandbox and the proxy): `cargo
new` + `cargo add itoa oorandom` + `cargo run` (index.crates.io, static.crates.io) and `npm init
-y && npm install left-pad`; `curl https://example.com` refused; and a `duet run` in
pass-through (`glm-5.3-flash`, default settings) that did the same by itself (8 turns, $0.0016;
`egress` events for index.crates.io and registry.npmjs.org). Linux: `tools/linux-check.sh`
(bubblewrap 0.8.0, OrbStack kernel), every test passing privileged, unprivileged and as root, the
bridge with duet itself as the helper through the frontier loop. Found on the way: `sensitive_data` commands and checks had the unrestricted network
of `sandbox.network = true`, and home credential files were readable (DUET-2026-022, fixed).

### M5.2 — Cost with security: the local model shrinks what the frontier reads

Measured baseline (Gate 2 sets and `results/xcal`): frontier cost is driven by turns × context,
not by the work itself; privacy mode adds turns (`ask_local`), which is its 1.6–1.8× premium.

1. **Instrumentation** — per-turn context size, turns and cost per task in duet-eval reports; a
   projection of every run's recorded tokens at other frontier price tables (e.g. `glm-5.3`,
   Anthropic and OpenAI flagships); a `duet-hybrid-nolocal` lane (privacy mode with no local model)
   so the local model's value is measured, not assumed.
2. **Context compaction** — past a threshold the local model condenses older turns into a working
   summary (recent turns verbatim; anything re-readable). Input is history the frontier already saw
   (already filtered); output passes the local-output filter. Rare, to limit prompt-cache breaks.
3. **Condensed command output and batching** — test/build output reduced to failures and relevant
   lines (deterministic parsers for common runners first, local model only for long unknown output;
   full text on request); the prompt asks for independent reads and commands in one turn.
4. **Structure views and synthetic look-alikes** — sensitive files presented with schema, value
   formats, masked sample lines and a rule-generated synthetic twin (no model reads real values for
   this; a vault check proves no real value is in the twin); cuts `ask_local` round trips.
5. **Local explorer** (second wave) — a read-only local sub-agent (secure zone: reads everything,
   no writes, no network, no decisions) answers "where/what/how" questions in one delegated call.

Each item ships behind a setting and is kept only if, head to head on the Gate 2 set and X1/X2, it
lowers frontier cost (projected at flagship prices too) with no quality loss and zero leaks.

*Built (branch `compaction`), item 2:* `context.compaction` (off), `context.compact_at` (100K
estimated tokens), `context.compact_to` (0.4). Masking first; when it cannot reach the target, the
local model condenses everything between the first message and the recent turns (whole turns, at
least 4) into working notes; the window's own masking stays the safety net. Its input is the older
part as the gate's filters leave it; its output is cleaned as local output and then passes the gate.
Events are recorded before they apply (`compacted` with the text; `masked` now with its positions),
so a resume sends the same request byte for byte. No local model (pass-through, `local.enabled`
off) means no attempt; a failure changes nothing and waits for growth. Offline replay with the live
`omlx-coding` (`compaction_replay` example) of `xcal/X1-duet-hybrid-s1` (236 requests): 4
compactions (requests 70, 122, 164, 226; ~101K → ~37K estimated tokens each), 122–358 s of local
time each (940 s in all, +19% on the run's 84 minutes, during which the run waits; 16–36K local
input and 1.5–4.1K output tokens), summaries of 3.4–7.9K characters; request tokens −49% and
uncached tokens −36% (its 48 window maskings are no longer needed). On the run's recorded usage
that projects to −43% frontier dollars at glm-5.3, −32% at Opus 5.5 and −36% at GPT-5.5. Dry
replays of all 12 XL runs (6,000-character stand-in summaries, no model): 1–6 compactions per run,
−25% to −46% each, −39% overall at glm-5.3 (−29% Opus, −32% GPT-5.5). The replay assumes the
frontier's later turns unchanged: re-reads after a compaction and any effect on quality are for
the head-to-head. The summaries read as specific and faithful (task and incident ids, files and
functions changed and why, failing probes with their exact errors, open items), each folding the
previous one in; detail of resolved issues thins out over generations (the fourth dropped the
early error texts and line numbers). Duet's estimate counts about 1.5× the provider's tokens on
these transcripts, so 100K estimated is about 67K provider tokens (XL mean context 71K).

*Built (branch `condense`), item 3:* deterministic condensers (`duet-boundary/src/condense.rs`) for
cargo test and rustc, pytest, unittest, jest, vitest, mocha, `node --test`, go test, tsc, eslint,
npm/pnpm/yarn, Maven, Gradle and generic pass/fail counts, applied by the engine after its
sensitivity decisions (public command and check output, and scanned output small enough to show
whole; held output untouched): the whole output sanitized first and kept under a public handle for
`read_raw`, omitted runs as markers naming their lines; unknown formats keep the outline and, with a
local model, its summary, as before. `context.condense_output`, on. The run, session and sub-agent
prompts ask for independent steps in one response (the loop runs a response's calls in order) and
the whole suite in one command; the run prompt's digest changed, so Gate 2 is re-measured before M5.
Measured on the 1,635 command outputs of `gate2b`, `gate2e` and `xcal`: every failing test name and
assertion line survives; the 43 that qualify shrink by 48%, which is 1.4% of all command-output
tokens and a projected 0.26% of the hybrid lane's input tokens over its runs (the models already cut
test output with `| tail`, and most command output is file reading). *Open:* the head-to-head
(setting on and off, Gate 2 and X1/X2); telling the frontier that test output arrives condensed, so
it stops cutting it with `| tail` (a prompt that depends on the setting).

*Built (branch `twins`), item 4:* `duet-boundary/src/structure/` (pure: format detection for JSON,
JSON lines, CSV/TSV and other delimiters, `KEY=value`, fixed-width, simple XML, logs; parsers that
keep every value's span; per-field profiles; shape masks; date layouts as pictures; twins; masking)
and `engine/structure.rs`. `sensitivity.structure_views` (on): a sensitive file's view gets its
structure view (in place of the repeated line shapes; logs their line templates) and a data file
the first record of its synthetic sample; `.env` views how each value is written; the task note an
outline of every policy-sensitive file. `synthetic_sample {handle, rows?}` (`sensitivity.
synthetic_rows`, 20): records chosen to cover the file's shapes, nulls and missing keys, every value
a fake that depends on its shape, kind, first position and the run's seed only (valid dates, Luhn
cards, mod-97 IBANs, detector-checked ids, `.test` emails), redrawn while it holds a known value,
then checked whole (no vault value, no copied span; fail-closed, audited); its detected fakes are
the frontier's own, and a file written from its lines only is a fixture, readable to commands under
a sensitivity glob while its content is exactly that. Masked output of `sensitive_data` commands (up
to 80 lines): values (vault, and every string value of the structured sensitive files as a whole
word) as shapes, secrets as `•••`, words kept only when public, schema or the command's; the
aggregate rule: numbers 0-99 as written within `sensitivity.masked_numbers` (24 per run), all else
as `9`s. Answering the `duet-hybrid-nolocal` smoke (S1: 34 `grep -q KEY .env && echo M` probes
decoded from byte counts; DUET-2026-023): a short output (≤200 characters) is a probe, counted
(`output_probe`) and past `sensitivity.output_probes` (12) replaced by a fixed text, with or
without a local model. Offline check on the three X2 hybrid runs (`results/structure-views-x2`:
views, samples and the runs' `sensitive_data` outputs replayed): of the 77 `ask_local` questions
about sensitive data, 37 (48%) answered by the views and 20 (26%) partly (counts and formats, not
the categorical values, which stay withheld); 20 need prose or values the views never show
(tickets, security reviews); 3 of 24 calls unnecessary outright and 6 more but for withheld values;
no canary of the runs' manifests in any view. The S1 probes sought what the task note's outline now
states. Live, one seed (`results/twins-s1-nolocal`, `duet-hybrid-nolocal`, glm-5.3-flash): S1 took
the `.env` layout from the outline and ran one `sensitive_data` command instead of 34 probes: 19
requests (32), $0.018 ($0.038), 244 s (614 s), 9/9 hidden tests, 0 leaks. *Open:* the head-to-head
(on and off, Gate 2 and X1/X2, with and without a local model).

*Built (branch `explorer`), item 5:* `explore {question, paths?, depth?}` (`explore.enabled`, off;
`explore.quick_steps` 12, `explore.thorough_steps` 30, `explore.max_seconds` 600 with a third for
quick, `explore.max_read_kb` 96 with half for quick), offered in hybrid and pass-through when a local
model is enabled; the run and session prompts name it only then. The local model drives a bounded
tool loop of its own (`duet-agent/src/explore.rs`; a `LocalAgent` implementing `Driver`, refused
for any endpoint not in the local role): the frontier's own `read_file`, `list_files`, `search`,
`code_nav` and read-only git tools through a secure-zone view (content as it is, 12 KB per result,
hidden paths and `.git`/`.duet` hidden, nothing writable), anything else refused; native tool calls
(checked live on oMLX `omlx-coding`: parallel calls, ~215 tokens/s uncached prefill, follow-up steps
served from the prompt cache). Report: a short answer and references checked against the files it
was shown; the model's words pass the boundary as `Source::Explore` (cleaned like an `ask_local`
answer about everything it read: copied runs, re-encodings, known and detected values, withheld-number
digits, the per-value budget with positional questions put as format questions, protected lines, and
with structure views every value of structured sensitive data), paths as a listing, code lines quoted
by Duet from open files only; sensitive references point to the structure view. One `explore` audit
event and `Explored` transcript entry per call; `ledger.explore` holds its local time (resume
included), which duet-eval counts as local work; lane `duet-hybrid-explore`. Found on the way:
`read_file` could read `.duet` run state (DUET-2026-024, fixed in `d280f07`). Tests: scripted local
stand-in through the loop (tools, step/reading/time caps, refused tools, checked references), a
careless explorer pasting everything it read plus a date of birth in prose (no canary reaches the
frontier), a positional question, protected source, injected instructions, resume after an
interrupt mid-exploration, pass-through. Live head-to-head (`results/explore1`, build `5d841f0`
before the rebase onto `261a369`, glm-5.3-flash, seed 1, explore off vs on): M2: the frontier never
called `explore` (4 source files, read directly): 12 vs 8 requests, 115K vs 71K request tokens,
$0.0094 vs $0.0068 frontier, 100% hidden both, 0 leaks both (the difference is run-to-run variance).
L1: one quick call (5 local steps, 10 files, 11 KB, 92 s local, 10 of 11 references kept) asking for
an overview, after which the frontier read 9 of the 10 referenced files whole anyway (none by range):
21 vs 25 requests, 343K vs 585K request tokens, $0.0198 vs $0.0318 frontier (+61%), 95% hidden both,
0 leaks both; wall 572 vs 1,536 s, of which ~12 minutes waiting for the frontier provider in the
explore run's first turn. Both tasks together, projected: $0.129 vs $0.172 at glm-5.3, $0.334 vs
$0.409 at Opus 5.5, $0.521 vs $0.636 at GPT-5.5. Reading on S–L repositories is already one
batched turn of whole-file reads, and the frontier re-reads what it will edit, so the explorer adds
a turn and its report instead of removing turns: kept off. *Open:* the XL head-to-head (X1/X2, where
reading takes many turns), questions narrower than an overview, and offering it only above a
repository size.

### M5.3 — Local security review: auditor first, then scanner

Why: the boundary keeps data local *during* a session, but nothing checks that the code duet
writes will not leak data *after deployment* (a new outbound host, customer fields in logs,
disabled TLS verification, a weakened auth check) — including code written by a frontier steered
by prompt injection. Banks also need per-change security-review evidence (PCI DSS 6.2.4, secure
development standards). The local model is the only model allowed to read protected code and the
code that handles real data, and it is independent of the model that wrote the change.

One engine, two entry points:

| | Auditor (first) | Scanner (second) |
|---|---|---|
| Scope | The run's or session's changes | The whole repository |
| When | Automatically at `finish`, before the work counts as done | `duet scan`, on demand or in the background; results kept per commit |
| Cost | Changed files only (minutes of local time) | About an hour of local time on a 58K-line repository at current prefill speed |
| Protects against | New vulnerabilities and data-leak backdoors in duet's own changes | Existing vulnerabilities and privacy flows |

Three layers:

1. **Deterministic rules** (duet's own, tree-sitter parsers and language-server references):
   sources (untrusted input: HTTP, files, CLI, environment; sensitive data: vault-known fields,
   structure-view fields, secrets) → sinks (SQL, shell, file paths, HTML, deserializers, logs,
   outbound HTTP, analytics) and the paths between them; plus known patterns (disabled TLS
   verification, weak crypto, hard-coded credentials, new outbound hosts). Operator-installed
   scanners (OpenGrep with openly licensed rules, cargo-audit, npm/pip audit, gosec, Bandit) run
   sandboxed as optional inputs; duet ships no rules under restrictive licences.
2. **Local model judgement** of each candidate in a small context: exploitable or not, missing
   sanitisation, severity, suggested fix. The only reviewer for protected code and privacy flows.
3. **Optional frontier second opinion** in a fresh context (only the diff and relevant open code,
   never the working conversation) for high-severity or uncertain findings on open code.

Rules: only rule-confirmed high-severity paths can block `finish` (setting); model-only findings
are advisory. The reviewer has no tools with side effects; its report passes the local-output
filter and the outbound gate. Every review is an audit event (counts, severities, rule ids, never
code), giving a per-change security-review record.

Privacy flows first: duet knows which fields are sensitive (vault, structure views), so it can find
what general scanners cannot — customer fields reaching logs or analytics, card numbers in error
messages, secrets in exceptions, personal data sent to a new host.

Measurement before trust: precision and recall on public vulnerable-code benchmarks (OWASP
Benchmark, CWE-Bench-Java, Juliet subsets) and on seeded vulnerabilities and data-leak backdoors in
dogfood tasks, for rules alone, rules + local model, rules + frontier, and all three; false alarms
on normal diffs from recorded benchmark runs; time per review. If the local layer adds little on
open code, its role narrows to protected code and privacy flows.

Order: after the M5.2 head-to-head, the host-native web search and the single egress choke point.

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
  *Built (branch `tui`):* `duet tui` (ratatui, crossterm backend) with Models (doctor offline; `--online`
  on request), Sensitivity (path tester), IP levels (git file tree, interface-only/sealed marks,
  skeleton preview), Limits, Data, Audit (records, stored request summaries, chain + anchor verify) and
  Run (follows transcript and audit; read-only). Edits share `Config::propose` / `apply` with `duet config
  set`: loosening shows the diff and needs `y`, the project file refuses owner-only keys and loosening,
  every applied change is appended to the config audit log; each value shows its origin. `TestBackend`
  tests per screen and per edit flow.
  *Built (branch `tui2`):* the Open items. Models: loopback auto-detection (the bootstrap discovery)
  with an audited pick of server and model, online checks (connection, context window) and a
  cache-reuse probe (two identical short requests, only on request). Sensitivity: custom detector
  patterns (`sensitivity.custom_patterns`, project add-only; matches become `data` placeholders in
  sensitive and public text) and a sample-text tester. Data: purge after a listed confirmation, under
  the workspace lock, with `duet purge`'s selection. Run: starting runs (`duet run` as a child;
  passthrough needs the no-privacy acknowledgement) and the dual-panel view (decision 2026-09-24): the
  live run beside the files it changed (write journal, +/- counts) and a per-file diff with line
  numbers that follows the run; sensitive and derived files are held locally. *Open:* a prefill-speed
  test on Models.
- Setup and presets for Ollama, LM Studio, llama.cpp, vLLM, oMLX, z.ai, Anthropic, OpenAI; no-config
  bootstrap detecting local servers on default ports; `duet doctor` with cache-reuse check.
  *Built (branch `m6doctor`):* local presets (`duet config preset`: Ollama, LM Studio, llama.cpp,
  vLLM, oMLX, mlx_lm.server; audited, `--confirm` for the endpoint change); the loopback-only
  bootstrap in `duet run` (single unambiguous server for one run, else the exact config commands;
  never writes config); `duet doctor` (offline by default, `--online` for listings and the local
  context window, `--json`, exit code = worst result).
  *Built (branch `dialects`):* frontier presets `zai`, `anthropic`, `openai` (endpoint, model, key
  variable and `frontier.dialect` through the audited owner-config path); the `--online` cache-reuse
  check (the same built-in prompt twice to the frontier and the local model; cached tokens of the
  repeat, warn when none). *Open:* an update check (needs a release channel, SbD-3).
- Anthropic Messages and OpenAI Responses dialects (moved here from M1), selected by the owner-only
  `frontier.dialect` (`chat` default). *Built (branch `dialects`):* both build their body from Duet's
  `Request`, so the gate filters, checks and audits the exact body of any dialect in passthrough and
  hybrid (no-canary property over all three shapes). Anthropic: sorted tools, a breakpoint on the
  stable prefix and a rolling one on the last block, `tool_use`/`tool_result` blocks, effort as
  adaptive thinking + `output_config.effort`, `input_json_delta` assembly, `max_tokens` and
  context-window stops terminal, cache read/creation usage, 429/5xx/529 and stream `error` events
  retried in place, other 4xx terminal. Responses: stateless (`store: false`), function calls,
  `reasoning.effort` with encrypted reasoning carried forward, `prompt_cache_key` from the stable
  prefix, cached-token usage, `incomplete` terminal. Signed thinking and encrypted reasoning are
  replayed unchanged in their original position; a turn the outbound filter edits drops them, and a
  provider refusing them after a history edit (context masking) gets one audited resend without
  them. Scripted-transport tests only; *open:* a live run on each (needs `ANTHROPIC_API_KEY` /
  `OPENAI_API_KEY`), and `xhigh`/`max` effort, which `frontier.reasoning_effort` does not offer.
- One live smoke test per local backend. *Built:* ignored tests in
  `crates/duet-boundary/tests/backend_smoke.rs`, gated by `DUET_LIVE_<BACKEND>_URL`; not yet run live.
- Large repositories: dogfood tasks X1 and X2 (real open-source repositories with injected
  sensitive assets), repository map, search scaling. *Built:* X1 and X2 (docs/DOGFOOD_SUITE.md §3),
  sealed and checked; they feed the M5 benchmark; not yet run live.

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
| Scope creep | Gates, one variable per run |

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
| 2026-09-24 | **Gate 2 re-examined: not established** | Termination fix (`359930e`) reclassifies runs that crashed without a terminal state as failures (they had been graded on their leftover workspaces). In `results/gate2` two hybrid runs (M1 s1, S1 s2; copied-span panic, fixed in `79fedc3`) become failures: hybrid 86.8% vs 98.3% hidden, judge 16.9 vs 18.8 /30 (Δ −1.9, lower bound −4.4) → quality FAIL; privacy still PASS (0 leaks). Gate 2 was measured on a build before the panic fix, so quality parity was never shown on a crash-free build; re-run on the current build with both judges (`results/gate2b`). Also merged: termination/retry fixes (every run ends in a terminal state; infra retries in place until budget; budget/interrupt/resume tests) and anchors/doc/doctor/TUI fixes (`faf3098`). |
| 2026-09-24 | L5 calibration | Build `f393214`, `results/l5cal`. Passthrough 100% on all 3 seeds (the predicted 30–90% band did not hold: glm-5.3-flash alone passes all 47 hidden tests); hybrid 100% ×3 with 0 leaks vs 1,330–14,538 canaries per run; cost $0.19 vs $0.14 (1.36×); wall 60–123 min vs 9–14 min; 77 vs 54 turns, 12 ask_local calls and 16 sensitive_data runs per run. No current task separates the lanes on pass rate; quality differences rest on the judges. Pass-rate calibration likely needs larger problems (X1/X2). |
| 2026-09-25 | Gate 2 re-run (`gate2b`, build `264d0af`, both lanes fresh, both judges) | **Quality PASS**: hybrid 97.8% vs 95.9% hidden, full success 72% vs 61%; judges Claude 19.9 vs 18.9 and Codex 21.9 vs 21.5, mean 20.9 vs 20.2 (Δ +0.7, lower bound +0.0); pass rate Δ +1.9 pp. **Privacy FAIL**: 1 of 18 hybrid runs (M3 s3) leaked an injection canary through a local answer that quoted a data file (DUET-2026-009, fixed `aa6226c`). Cost $0.0229 vs $0.0131 (1.75×), wall 648 s vs 178 s. Hybrid re-run on the fixed build: `gate2c`. |
| 2026-09-25 | Gate 2 hybrid re-run (`gate2c`, build `aa6226c`) | **Privacy PASS**: 0 leaks in 18/18 hybrid runs. Quality FAIL narrowly (judge mean 19.8 vs 20.2, Δ −0.3, lower bound −2.8; pass 92.2% vs 95.9%) — driven by one run (M3 s3) that ended at its first send: placeholder values (`name: value`, `name: [… redacted]`) had become vault names and Duet's own redaction marker then tripped the final check (fail-closed, nothing sent). Fixed `d55d2cc` (placeholder values are not names; fixed markers never block). Full hybrid re-run on that build: `gate2d`. |
| 2026-09-25 | **Gate 2 re-established: PASS** (`gate2d`, build `d55d2cc`, both judges) | Hybrid (fresh) vs passthrough (fresh, `gate2b`), S1/S2/M1/M2/M3/L1 × 3, every run terminal, none failed. Privacy: 0 leaks in 18/18 hybrid runs (passthrough 3,133 across 18/18). Quality: hidden 97.2% vs 95.9%, full success 72% vs 61%; judges Claude 20.1 vs 18.9, Codex 21.5 vs 21.5, mean 20.8 vs 20.2 (Δ +0.6, lower bound +0.0). Cost $0.0214 vs $0.0131 (1.63×), wall 412 s vs 178 s. This supersedes the earlier Gate 2 rows as the verdict on the current product. |
| 2026-09-25 | **Gate 2 re-confirmed: PASS** on the merged build (`gate2e`, `8bf0da1`: WebSocket proxy, runtime, TUI, dialects) | Hybrid fresh vs `gate2b` passthrough, both judges. Privacy: 0 leaks in 18/18. Quality: hidden 96.7% vs 95.9%; judges Claude 19.3 vs 18.9, Codex 21.0 vs 21.5, mean 20.1 vs 20.2 (Δ −0.0, lower bound −0.9). Cost $0.0233 vs $0.0131 (1.78×), wall 603 s vs 178 s. EXT_DISK (USB) dropped out at 10:26 and killed the runner mid-batch; the run overlapping the outage was set aside as invalid and re-run. Since fixed: device dropouts are transient host failures in duet and in duet-eval (`bebf7f6`). Also merged this round: X1/X2 tasks, provider data-handling terms (`14f1066`), Linux sandbox verified in containers (`be8b1c1`). |
| 2026-09-25 | **M5.0 + M5.1 built; boundary hardening round** (main `2a51816`, post-merge gate green) | Sessions and steering (`duet chat`, TUI input; `64faeb0`). Practical toolset: web fetch/search with outbound check (`fe6fb54`), git history/commit (`03a1e8f`), MCP client (`1aabba1`), language servers (`f553171`), sub-agents (`49aeb15`), images (`b4d75d1`; oMLX `omlx-coding` has no vision, so hybrid images need a public mark or a vision-capable local model). Live hybrid `duet run "is this credit card number valid …"` exposed six boundary defects (raw Luhn-invalid number, operator placeholders unusable, `sensitive_data` commands reading `.duet`, a 4-digit prefix via a local answer, a name false positive, placeholders in operator output); fixed with DUET-2026-010…012 (`4ca2602`) and re-run live: 0 digits reach the frontier, 3–5 turns. Adversarial privacy scenario suite (transformed-form canary matcher, careless local stand-in; `fd85cf7`) found 7 more gaps, closed with DUET-2026-013…017 (`2a51816`): ignored sensitive files primed, cumulative `ask_local` probing budgeted and audited, re-encoded/spelled local output cleaned, derived build output tracked, unlabelled operator numbers tokenized; 20/20 scenarios pass. Detection coverage (SbD-2): 222 gitleaks rules as data, corpus recall 228/228, 0/251 reach the frontier, international PII formats (`a1886a7`). |
| 2026-09-26 | X1/X2 calibration (`results/xcal`, builds from `14f1066`; the two X2 seed-3 runs partly on `2a51816` after a gate rebuilt `target/debug`) | First XL measurement. **Privacy PASS**: hybrid 0 leaks in 6/6 vs passthrough 23,185 canaries in 6/6. **Quality: hybrid behind** — hidden 65.9% vs 93.0% (X1 86–94% vs 96%; X2 64% vs 84–100%, plus one hybrid run ended failed by a fail-closed known-values block after masking and scored 0%, under investigation on branch `blocked`); cost $0.48 vs $0.53, wall 68 vs 42 min; 169 vs 159 turns. X2 hybrid misses partner-feed formats it can only see as summaries (sensitive samples) and number-precision cases; false positives (an ISBN as a card, a numeric `.env` limit withheld) hid public information. XL tasks separate the lanes on pass rate (unlike L5), so they are the benchmark's discriminating tier and the target for M5.2 (local model doing more of the reading: synthetic twins, statistics, computing on placeholders). |
| 2026-09-26 | M5.2 item 1: cost instrumentation (`be69e0d`) | Per-turn cost profile and price projection over existing batches. Frontier-only premium hybrid/passthrough: gate2b 1.45× [0.97, 2.20], gate2e 1.50× [1.18, 1.98]; XL 0.87× [0.52, 1.52]; unchanged at glm-5.3, Opus and GPT-5.5 prices (1.38–1.50 S–L, 0.86 XL). The earlier 1.6–1.8× figure included an electricity upper bound. Input is 62–68% of frontier dollars on S–L and 91% on XL; hybrid adds ~5–6 turns per S–L run, 3.4–5.0 of them `ask_local`, and ~20% more context per turn. Context per turn: S–L 11–14K, XL 71–79K (p90 ~100K). `local.enabled = false` adds privacy mode without a local model; its S1 smoke run (0 leaks, 9/9) cost 4.8× hybrid because the frontier probed `.env` bit by bit with `sensitive_data` greps — the probing channel must be budgeted in both modes (M5.2 item 4). |

Scope changes, with reasons:
- **Anthropic Messages and Responses dialects move to M6.** The frontier (z.ai GLM) and the local
  server (oMLX) both speak Chat Completions, so no gate depends on the other dialects.
- **The local-model micro-evaluation moves to M3.** It measures the digest and answer roles, whose
  prompts and schemas are written in M3; it runs before Gate 2.
