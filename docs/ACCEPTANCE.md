# Duet v2 — Acceptance and Requirements Traceability

Status: audit of `main` at `234165b` (2026-09-24), before any release. It is read-only: no code was
changed, and no models or evaluations were run. The evidence is the test suite (one full
`tools/gate.sh` run), the measured batches under `results/` (EXT_DISK), the code, and the git history.
The operator's standing decisions are in [PLAN.md](PLAN.md) §3. Rows and defects marked "after the audit" were
updated later on 2026-09-24 (branch `nits`) with the fix and its evidence; the rest is unchanged.

The question it answers: **does Duet work, and does it meet its requirements?** Short answer:
the core privacy claim is well supported by tests and by live measurements (0 canaries in 74 valid
hybrid runs, and every outbound request appears in Duet's audit log). Quality was last shown
non-inferior at build `a5346f4` (Gate 2, one judge). It has not been re-established on the current
build, and the second judge has never run. Termination, resume, five of the six local backends,
the Linux sandbox and large repositories are either untested or unmet. Several documents claim
more than the evidence shows. A release is **not** acceptable yet; the blockers are in §4.

Update (branch `term`, 2026-09-24): the termination and retry rows T1–T5 and EV10 were fixed and
re-verified by automated tests (blockers 2 and 3, defects D1, D2 and D4). Their rows, the counts
in §4.1 and the notes in §4.2 and §4.4 say so; nothing else was re-audited.

Update (branch `runtime`, 2026-09-25): the gaps left in T2 and T3 were closed with tests: a full
disk pauses a run instead of failing it, the estimated usage of failed attempts is charged, and
Ctrl-C kills a running command's process tree. The config-change audit helper that the CLI and
the TUI each had is now one function (CF4, SD2 unchanged in behaviour). Only rows T2 and T3 changed.

Update (branch `linux`, 2026-09-25): the Linux sandbox was run for the first time
(`tools/linux-check.sh`), fixed and verified (blocker 5). Only rows P7, SD4 and SD5, the claim
map in §3, the counts in §4.1 and blocker 5 changed.

## 1. How to read this

| Verdict | Meaning |
|---|---|
| **Verified** | An automated test in the gate and/or a measured live run proves it (cited) |
| **Partial** | Some of it is proven; the gap is stated |
| **Unverified** | Implemented, but no test or run has ever exercised it |
| **Not met** | Measured or inspected, and it does not hold |
| **Not implemented** | Not built (including items planned for M5/M6) |

Test names are Rust test functions (`cargo test <name>`). Batches are directories under `results/`,
each with a `report.md`. The build each batch ran on is in `results/<batch>.build`. Batch summary:

| Batch | Build | Lanes (runs) | What it measured |
|---|---|---|---|
| `pilot-pi-flash` | — | pi-glm (70, 35 invalid: quota) | M0.4 pilot, calibration |
| `gate1-flash` | M2 | duet-passthrough 20, pi-glm 20 | Gate 1 (harness health) |
| `m3-hybrid` | M3 | duet-hybrid 18 | Before the leak fixes: 5/18 runs leaked (103 occurrences) |
| `m3-hybrid-v2` = `gate2` hybrid | `a5346f4` | duet-hybrid 18 | Gate 2 verification batch |
| `gate2` / `gate2-passthrough` | `a5346f4` | duet-passthrough 18 | Gate 2 baseline |
| `cost1-hybrid` | `a5346f4` | hybrid 15, passthrough 15 | Cost baseline |
| `gate3a` / `gate3b` / `gate3c` | `aeda66e` / `9a8026c` / `4384ca0` | hybrid 12 each, passthrough 12 (shared) | Gate 3 attempts A–C |
| `l2l4` | `a67ae67` | hybrid 7 (1 invalid, disk full), passthrough 6 | IP gate (L2), L4 calibration |
| `l5cal` | `f393214` | hybrid 1, passthrough 2 | L5 calibration (**in progress** during this audit) |

## 2. Gate run (this audit)

`CARGO_TARGET_DIR=target/dev tools/gate.sh` on `234165b`, 2026-09-24: **all checks passed** (exit 0).

| Step | Result |
|---|---|
| format (`cargo fmt --check`, fuzz targets) | pass |
| lints (`clippy --workspace --all-targets --locked -D warnings`) | pass |
| tests (`cargo test --workspace --locked`) | **297 passed, 0 failed, 8 ignored** |
| dependency policy (`cargo deny check`) | advisories ok, bans ok, licenses ok, sources ok |
| license headers, privacy by construction, provenance | pass |

The 8 ignored tests are the live tests, and no gate ever runs them. Six are
`duet-boundary/tests/backend_smoke.rs` (`ollama`, `lmstudio`, `llamacpp`, `vllm`, `omlx`, `mlx`), which
have **never been run live**. Two are `live_glm_tool_round_trip_and_cache` and
`live_local_tool_round_trip_and_prefix_reuse`, run once by hand for M1 (PLAN §10 M1 row; no log is
kept). The following were also run: `duet-eval selftest` ("selftest ok: canaries, proxy, statistics,
pricing"), which is offline and uses a dead upstream, and `duet-eval validate` (all 11 task packages
valid and sealed).

Checks this audit added on the stored results (read-only scripts, in
`/Volumes/EXT_DISK/duet_v2/scratch/acceptance/`):

- **Audit completeness.** For every Duet-lane run directory with a proxy log (227 directories;
  some batches share runs), each of the 6,081 request bodies the leak proxy recorded has a matching
  `request_sha256` in Duet's own audit log. Missing: 0.
- **Terminal states.** Of 141 live Duet run summaries, 137 are `completed` and 4 `failed`. Four
  distinct runs (six directories) **panicked** (exit 101, no summary, no terminal state):
  `m3-hybrid/M3-duet-hybrid-s3`, `gate2/M1-duet-hybrid-s1` and `gate2/S1-duet-hybrid-s2` (both also in
  `m3-hybrid-v2`), and `gate3a/L3-duet-hybrid-s3`. All four hit the same panic at `overlap.rs:105`
  (DUET-2026-005, fixed `79fedc3`). No live run ever ended in `BudgetStopped`.
- **Prompt caching.** Cache reads are 90–96% of frontier input tokens in `gate2`, `gate3c` and
  `l2l4`, for both lanes.
- **Anchor verification.** `duet audit verify` on `l2l4/L2-duet-hybrid-s1` reports "chain intact: 34
  records". It then fails (exit 2) with "no anchor", because the anchor is filed under a hash of the
  workspace path, and the results were moved to EXT_DISK after the run (defect D3). *Resolved
  after this audit (branch `nits`): anchors are now keyed by run id and first record, and this run
  now verifies (exit 0); see P16.*

## 3. Requirements

### 3.1 Quality

| ID | Requirement | Source | Verdict | Evidence | Gap / next action |
|---|---|---|---|---|---|
| Q1 | Non-inferior to the same frontier alone: judge 95% lower bound > −2/30, and not behind on hidden pass rate on a majority of tasks | TARGET §1; PLAN §3 2026-09-23 (rule) | **Partial** | Gate 2 PASS (`gate2`, build `a5346f4`, 18 pairs): judge Δ +0.1, lower bound −0.8; pass rate 97.9% vs 98.3%, behind on 1/6 tasks. One judge (Claude). No later batch was judged. Pass rate only: `gate3a` behind on 2/4 tasks, `gate3b` behind on **3/4** (fails the majority rule for that build), `gate3c` 1/4, `l2l4` 1/2 (n=6) | Quality at HEAD is unmeasured: M4 bulky offload, M4.5 IP and later changes all came after Gate 2. Run M5 (`duet-hybrid` vs `duet-passthrough`, S0–L5, both judges) on the release build |
| Q2 | Gate 1: `duet-passthrough` is non-inferior to `pi-glm` on the same model | PLAN §5 M2 | **Verified** | `gate1-flash` (20 pairs): judge Δ +0.8, lower bound −0.2 → PASS; behind on 1/4 tasks | — |
| Q3 | M5 is judged by two families (Claude CLI and Codex CLI); gates use the mean; self-family runs are flagged | PLAN §3 2026-09-24 | **Partial** | Implemented and unit-tested: `gates_use_the_mean_of_both_judges`, `agreement_and_self_judged_lanes_are_reported`, `the_benchmark_requires_both_judges_on_every_run`, `judges_are_two_families_and_the_api_is_the_claude_judge`, `judge_defaults_to_both_judge_families`, `the_judge_never_uses_the_evaluation_codex_home` | The Codex judge has never run: no batch has a Codex judge file. Run `duet-eval judge` with both judges on one batch first |
| Q4 | Public benchmark `docs/BENCHMARK.md`, regenerated byte for byte from the raw data | PLAN §5 M5 | **Partial** | `report --final` implemented: `output_regenerates_byte_for_byte`, `pairs_runs_across_batches_and_applies_the_current_rules`, `changing_a_raw_record_changes_the_digest`, `a_run_given_twice_is_refused` | Never produced; `docs/BENCHMARK.md` does not exist (M5) |
| Q5 | The quality cost of protected-code edits is measured and published | TARGET §1 (IP); PLAN M4.5 | **Partial** | `l2l4`: L2 hybrid 100% hidden in 3/3 runs, with 3, 2 and 3 `edit_protected` calls (audit logs); passthrough 100% | Not judged, n=3, not published. Include L2 in M5 with both judges |
| Q6 | Calibration: the frontier-only reference passes 30–90% of each task's hidden tests | PLAN M0.4; DOGFOOD_SUITE §2 | **Not met** | M1–M3 at 100% on every seed (PLAN §10 pilot row); L4 passthrough 100% (`l2l4`); L5 passthrough 100% (`l5cal`) | Documented in PLAN §10. With this frontier, most tasks separate lanes only through the judge. X1/X2 and harder tasks are needed |
| Q7 | Local model chosen by `duet local-eval`: accuracy ≥ 0.90, error-line recall ≥ 0.95, schema ≥ 0.99, 0 leaks | TARGET §10; PLAN §3 2026-09-23 | **Partial** | `results/local-eval/omlx-qwen-oneschema.json`: accuracy, evidence, schema and digest 1.00, 0 leaks, cache reuse 0.92 (15 fixtures). Tests: `scoring_is_exact_and_unanswerable_aware`, `leaks_are_counted`, `fixtures_are_deterministic_and_plant_their_fact` | 15 fixtures cannot establish ≥ 0.99 schema validity. The file records `"pass": false` (prefill was gated then; ungated by decision). Measured 2026-09-23, before the brief and implement roles changed the local prompts. Re-run on the release build with more fixtures |

### 3.2 Agent loop and context

| ID | Requirement | Source | Verdict | Evidence | Gap / next action |
|---|---|---|---|---|---|
| A1 | A real agent: one append-only conversation, a byte-stable system prompt, a fixed sorted tool list | TARGET §2, §4; north star | **Verified** | `specs_are_sorted_and_stable`, `body_is_stable_sorted_and_replays_raw_arguments`; live cache-read share 90–96% (§2) | — |
| A2 | Tool set: `read_file`, `list_files`, `search`, `diff`, `edit_file`, `write_file`, `run_command`, `ask_local`, `read_raw`, `finish`, `edit_protected` | TARGET §4 | **Verified** | All 11 are defined (`duet-agent/src/tools.rs`, `duet-boundary/src/engine*.rs`); per-tool tests below; all used in live hybrid runs | — |
| A3 | `edit_file` uses Duet's own `edits[{old,new}]`, applied all-or-nothing | TARGET §4; PLAN §2.3 | **Verified** | `edits_apply_all_or_nothing` | — |
| A4 | A length-truncated response never runs its tool calls; `length` and `content_filter` are terminal, not retried | TARGET §4; PLAN M1 fix 3 | **Partial** | `length_stop_is_reported_and_blocks_tools`, `length_stop_is_terminal_not_retried`, `missing_finish_is_truncation_and_missing_usage_is_estimated` | `content_filter` → `Failed` (`run.rs`) has no test |
| A5 | Masking at `context.mask_at` (70%) in whole turns, last turns kept, stubs name the call and handle; a tool call is never separated from its result | TARGET §5; ARCH §12 inv. 6 | **Verified** | `masks_oldest_turns_first_keeps_recent_and_drops_below_half`, `a_turn_is_masked_whole_and_every_call_keeps_its_result`, `stubs_name_the_call_and_the_handle`, `nothing_masked_below_threshold`; masking happened in 6 of 141 live runs | — |
| A6 | `ask_local` takes ≤ 6 questions and answers in a fixed schema; invalid output is retried once, then reported unavailable, never invented | TARGET §6.3 | **Partial** | `every_role_shares_one_schema`, `extracts_json_from_fenced_or_prose_output`; used live (5.8–12 calls per run, reports) | The retry-once-then-unavailable path (`local.rs:91`) has no test |
| A7 | `read_raw` returns ranges of public bulky handles only (≤ 500 lines); bulky results show head + outline | TARGET §4, §6.2 | **Verified** | `bulky.rs` (13 tests, e.g. `a_bulky_file_becomes_a_handle_with_head_and_outline`, `bulky_search_keeps_sensitive_matches_masked`, `the_task_is_never_offloaded`) | — |
| A8 | `finish` runs `checks.commands` sandboxed; the frontier may continue up to `limits.max_finish_attempts` | TARGET §4 | **Partial** | Used in every live run; checks output shaping: `runs_that_can_read_protected_code_show_result_lines_only` | No test of the finish-attempt limit or of failed-check continuation |

### 3.3 Privacy (sensitivity)

| ID | Requirement | Source | Verdict | Evidence | Gap / next action |
|---|---|---|---|---|---|
| P1 | Zero planted canaries from sensitive sources in outbound traffic, verified by an independent logging proxy | TARGET §1; SECURITY Verification | **Verified** | 74 valid hybrid runs after the leak fixes (`m3-hybrid-v2`/`gate2` 18, `cost1-hybrid` 15, `gate3a` 12, `gate3b` 12, `gate3c` 10, `l2l4` 6, `l5cal` 1): **0 leak occurrences**. Exact binomial 95% upper bound on the per-run leak rate ≈ 4.0%. Passthrough on the same tasks leaked in every run (e.g. 3,130 occurrences in `gate2`). Proxy: `records_forwards_streams_and_flags_canaries`, `refuses_request_bodies_it_cannot_scan`, `finds_raw_json_escaped_and_variant_forms` | The latest measured builds are `a67ae67` (6 runs) and `f393214` (1 run), not HEAD. Re-measure in M5 on the release build |
| P2 | 100% of outbound bytes are in a hash-chained audit log | TARGET §1 | **Verified** | This audit: 6,081/6,081 proxy request hashes found in Duet audit logs (§2). `chain_verifies_and_detects_tampering`, `logs_written_before_events_still_verify` | The cross-check is not automated. Add it to `duet-eval run` so every batch proves it |
| P3 | One door out: the agent cannot construct an ungated frontier provider | TARGET §3.2; ARCH §12 inv. 1 | **Verified** | The gate step "privacy by construction" (`duet-agent` does not depend on `duet-provider`); `GatedFrontier` is built only by `OutboundGate::wrap` (`gate.rs:66`) | — |
| P4 | Outbound gate on every request and every role (including the frontier's own text, reasoning and tool arguments): re-tokenize, re-scan, copied-span filter, final known-value check that blocks; the filter leaves nothing the check refuses, and a part the check still refuses is withheld instead of ending the run | TARGET §6.4 | **Verified** | `no_canary_survives_the_gate` (property), `the_check_passes_what_the_filter_returns` (property: filter ∘ check never blocks, every dialect, with framing; withholding alone clears any request), `a_value_the_filter_finds_in_a_later_item_is_replaced_in_earlier_ones`, `a_vault_value_that_spells_the_wire_format_blocks_nothing`, `a_value_found_in_a_masking_stub_is_replaced_in_the_call_it_quotes`, `a_part_the_check_refuses_after_filtering_is_withheld_and_the_run_goes_on`, `a_request_withholding_cannot_clear_ends_the_run_with_the_reason`, `outbound_filter_and_check_stop_known_values`, `the_models_own_messages_are_sanitized_too`, `values_escaped_in_tool_arguments_are_replaced_and_checked`, `a_value_inside_a_longer_detected_span_is_known_alone`, `overlap_redaction_leaves_no_copied_run`, `a_blocked_send_is_audited_by_check_name_only`. Live fail-closed blocks: `gate3a/S1-duet-hybrid-s2`, `gate3c/S2-duet-hybrid-s2/-s3`, `xcal/X2-duet-hybrid-s2` (DUET-2026-019, reproduced offline from its transcript and vault) | — |
| P5 | Classification layers: path, source, secret, PII, sensitive text, taint; any layer can mark sensitive, none can unmark | TARGET §6.1 | **Verified** | `detect.rs` (7), `globs_match_names_anywhere_and_paths_exactly`, `command_output_is_sensitive_unless_allowlisted`, `person_fields_are_replaced_whatever_their_shape`, `logs_become_a_handle_with_sanitized_error_lines`, `secrets_in_public_source_are_replaced`, `commands_cannot_read_sensitive_files_unless_their_output_stays_local` (taint) | — |
| P6 | Placeholder vault: stable tokens; aliases for other spellings, never written back; values the frontier wrote are left alone | TARGET §6.2 | **Verified** | `stable_tokens_round_trip`, `repeated_values_keep_one_token_across_sources`, `numbers_are_replaced_in_their_other_spellings`, `surnames_are_replaced_alone_unless_the_word_is_public`, `values_the_frontier_wrote_are_shown_as_written`; properties `vault_round_trips`, `tokenize_leaves_no_value`, `tokenize_is_idempotent`, `aliases_never_detokenize` | — |
| P7 | Commands cannot read sensitive paths, protected source, `.git` or `.duet` (OS sandbox) unless run with `sensitive_data` | TARGET §3.2; SECURITY "Still prevented" | **Verified** | macOS Seatbelt (gate) and Linux bubblewrap: `denied_paths_cannot_be_read_by_any_means`, `denied_paths_stay_denied_through_symlinks`, `git_and_run_state_are_unreadable_when_denied`, `commands_cannot_read_git_history_or_run_state`, `commands_cannot_read_sensitive_files_unless_their_output_stays_local`, `protected_edits_are_implemented_locally_and_checked`, `protected_source_is_hidden_from_commands_but_not_from_checks`, `bwrap_args_protect_reserved_dirs_and_deny_reads`; fail closed: `commands_are_refused_when_namespaces_are_unavailable`, `commands_are_refused_when_bubblewrap_is_missing`. Linux run (`tools/linux-check.sh`, 2026-09-25): image `rust:1-bookworm` (`sha256:93ce27a88655…`) + Debian bubblewrap 0.8.0, rustc 1.98.1, kernel 7.0.14-orbstack (AArch64, OrbStack VM); `duet-sandbox` 20/20, `duet-agent` 24/24, `duet-boundary` all passing in each of: privileged; unprivileged (non-root, no added capabilities, `seccomp=unconfined` + `systempaths=unconfined`, i.e. unprivileged user namespaces); as root (`commands_hold_no_capabilities`). Docker's default profile (no namespaces): commands refused, `real_bubblewrap_without_namespaces_refuses`. Live: 1.2–2.2 sandbox denials per hybrid run (ledger, macOS) | Linux is not in the gate: run `tools/linux-check.sh` before releases. Not yet run on a distribution host with AppArmor user-namespace restrictions (e.g. Ubuntu 24.04; there Duet works or refuses). Found and fixed on the first run (branch `linux`): every Linux command failed (`--chdir` came after `--`); denied paths were empty but readable, nested `.git` writable, Unix sockets reachable with network off, and commands of a root-run Duet held every capability. Linux limits (new `.git`/`.duet` removed after the command rather than refused; per-command workspace walk): SECURITY "Command sandbox by platform" |
| P8 | Secret-sink check: resolved secrets land only in owner-listed sinks or sensitive files | TARGET §6.5 | **Verified** | `secrets_are_restored_only_in_secret_files`; the grader's `finds_secret_outside_sinks_only`; 0 sink violations in every batch | — |
| P9 | The local role accepts only loopback or owner-allowlisted hosts; plain HTTP to a non-loopback host is refused unless `local.allow_plaintext` | TARGET §3.2; SbD-1 | **Verified** | `local_role_admits_loopback_and_allowlist_only`, `plaintext_to_a_remote_host_needs_the_owner_opt_in`, `local_role_refuses_non_loopback_endpoints`, `a_remote_local_model_over_plain_http_is_refused`, `doctor_fails_a_refused_local_endpoint_and_does_not_contact_it` | Note: every live hybrid run used the operator's opt-in, so raw sensitive content crossed the LAN in plain HTTP to `192.168.50.132` (by owner decision, PLAN §9) |
| P10 | Injection canaries never cross; prompt-injection residual risk documented | PLAN M3; SECURITY | **Verified** | M3 `hostile-logs`: hybrid 0 injection canaries, passthrough 91 (`gate2`); SECURITY "Prompt injection: residual risk" | — |
| P11 | Red-team pass on the boundary (fresh canaries, encodings, injection) before publishing the benchmark | PLAN SbD-2; §3 2026-09-24 | **Not implemented** | Parked until last (operator decision) | Required by the SbD gate before `docs/BENCHMARK.md` |
| P12 | Fuzzing of the parsers and boundary blocks, "run before releases" | PLAN SbD-2; SECURITY Verification | **Unverified** | 5 targets exist (`fuzz/fuzz_targets`: sse, chat_recover, vault, overlap, detect; `763db74`); `tools/fuzz.sh` | No recorded fuzz run (no corpus, no log, no duration). Run `tools/fuzz.sh` for a stated time per target and record it |
| P13 | Property tests in the gate | PLAN SbD-2 | **Verified** | `duet-boundary/tests/properties.rs` (9), `no_canary.rs` (1), `duet-provider/tests/properties.rs` (5); all in this gate run | Only small case counts in the gate; record one `PROPTEST_CASES=5000` run before a release |
| P14 | Local data hygiene: `.duet/runs/<id>/` mode 0600, `duet purge` by retention; automatic deletion | TARGET §9 | **Partial** | `private_modes_and_torn_tail_repair`, `persists_privately`, `snapshots_are_private_and_readable` | `duet purge` has no test. Automatic deletion is **not implemented** (M6, documented) |
| P15 | Per-run disclosure report: counts and kinds only, never values | PLAN SbD-3 | **Verified** | `counts_withheld_content_by_class_without_values`, `passthrough_runs_are_reported_as_unprotected`, `audit_disclosure_reports_counts_from_the_log_and_the_summary` | — |
| P16 | Security events in the audit chain; head anchored outside the workspace; `duet audit verify` detects a rewrite | PLAN SbD-1 | **Verified** (updated after the audit) | `events_join_the_chain_without_content`, `anchor_detects_a_rewritten_or_truncated_log`, `audit_verify_checks_the_anchor_and_show_lists_events`, `doctor_verifies_recent_audit_logs_and_anchors`. D3 fixed on branch `nits`: anchors are filed under `audit-anchors/runs/<run-id>/<first-record hash>.json`, so they no longer depend on the workspace path; `anchors_survive_a_moved_workspace` (verify matches after a move; a re-chained log, with or without its first record, and an emptied log are still reported), `anchors_in_the_earlier_layout_are_still_read` (old-layout anchors are found in place and after a move) | — (after the fix, `duet audit verify` on the stored `l2l4/L2-duet-hybrid-s1` reports "chain intact: 34 records" and "anchor matches" against its earlier-layout anchor, exit 0) |
| P17 | Run start names the sensitive paths; the local brief is off by default | TARGET §4; PLAN §3 2026-09-24 | **Verified** | `the_task_names_the_sensitive_paths`, `the_task_carries_a_cleaned_local_brief_of_the_sensitive_files`; registry default `sensitivity.local_brief = false` | — (README corrected after the audit: the brief is described as optional, off by default) |

### 3.4 Intellectual property

| ID | Requirement | Source | Verdict | Evidence | Gap / next action |
|---|---|---|---|---|---|
| IP1 | Zero IP canaries (protected function bodies) in outbound traffic | TARGET §1, §7; PLAN M4.5 gate | **Verified** | `l2l4`: L2 hybrid 0 leaks in 3/3 runs, including source canaries; passthrough 102 source-canary hits. `copies_of_protected_code_do_not_cross_the_gate`, `lines_containing_protected_code_are_withheld` | n=3 (binomial upper bound 63%). Enlarge in M5 |
| IP2 | Interface-only: tree-sitter skeletons (Rust, TypeScript, Python), bodies as handles; unparsable files treated as sealed | TARGET §7 | **Verified** | `rust_skeleton_keeps_the_interface_and_withholds_bodies`, `typescript_…`, `python_skeleton_keeps_docstrings_and_withholds_bodies`, `skeletons_are_deterministic_and_reject_broken_files`, `interface_only_files_show_a_skeleton_with_stable_handles` | — |
| IP3 | Sealed paths show existence only; listings, searches and diffs withhold protected content; the frontier cannot write protected files directly | TARGET §7 | **Verified** | `sealed_files_show_existence_only`, `searches_listings_and_diffs_withhold_protected_content`, `protected_files_cannot_be_written_directly` | — |
| IP4 | `edit_protected`: frontier spec + tests → local implementation → host runs checks → pass/fail and result lines only | TARGET §7 | **Verified** | `protected_edits_are_implemented_locally_and_checked`, `the_local_model_implements_protected_changes`, `implementations_that_do_not_parse_are_refused`, `runs_that_can_read_protected_code_show_result_lines_only`, `only_result_shaped_lines_survive`; live: 2–3 per L2 run | — |
| IP5 | IP levels settable from the TUI, with a skeleton preview | TARGET §8 | **Verified** | `ip_screen_marks_paths_and_previews_the_skeleton`, `ip_marks_become_duet_project_config` | — |

### 3.5 Cost

| ID | Requirement | Source | Verdict | Evidence | Gap / next action |
|---|---|---|---|---|---|
| C1 | Strictly cheaper than the frontier alone (original Gate 3) | PLAN §3 2026-09-23 | **Not met** | Attempts A–C failed (`gate3a`–`gate3c`); superseded by the 2026-09-24 decision | Recorded for completeness; see C2 |
| C2 | Report a measured privacy premium: the paired cost ratio vs passthrough (same model) with its 95% interval | PLAN §3 2026-09-24; TARGET §1 | **Partial** | `paired_ratio_is_the_ratio_of_totals` (used only by `--final`). Batch reports show the paired **difference** and its bound, not the ratio. Ratios of lane means in this audit: `gate2` 2.4×, `cost1-hybrid` 1.9×, `gate3a` 1.7×, `gate3b` 1.5×, `gate3c` 2.2×, `l2l4` 2.1× | No ratio with an interval has been published. Publish the ratio with its interval from M5. (After the audit, README, TARGET and PLAN §5 report the measured 1.5–2.4× instead of "~1.4×"; the PLAN §3 decision row keeps its original text) |
| C3 | Cost = list price including cache reads and writes + local electricity | TARGET §1 | **Partial** | Prices verified with source and date (`selftest`; `prices_refuse_unverified_and_compute_cost`, `prices_usage`); cache-aware usage (`openai_chat_stream_usage`, `anthropic_stream_usage`) | Electricity is **estimated** as `local.watts` × the run's wall seconds (`lanes/mod.rs`), not measured and not busy seconds (PLAN M0.2 says busy seconds). No price row for the `codex` lane's model |
| C4 | Per-run cost ledger by content class in `summary.json`, shown per lane | PLAN M4 | **Verified** | `bulky_files_reach_the_frontier_as_a_preview_and_are_charged_as_such`, `passthrough_shows_the_whole_file_and_charges_it_as_raw`, `results_are_charged_to_their_class_for_every_request_that_carries_them`, `lanes_with_a_ledger_get_a_cost_breakdown`; ledgers in `gate3a`–`l2l4` reports | — |
| C5 | Usage is never poisoned by retries; missing usage is estimated | PLAN M1 fixes 1, 6 | **Verified** | `retried_failures_do_not_poison_usage`, `missing_finish_is_truncation_and_missing_usage_is_estimated` | — |
| C6 | Provider prompt caching works on the frontier | PLAN §3 (resolved 2026-09-23) | **Verified** | `cached_tokens` 0 → 28,544 (PLAN §3); live cache-read share 90–96% (§2) | — |

### 3.6 Termination and resume

| ID | Requirement | Source | Verdict | Evidence | Gap / next action |
|---|---|---|---|---|---|
| T1 | Every run ends as `Completed`, `Failed{reason}` or `BudgetStopped` | TARGET §1; operator one-shot rule; ARCH §12 inv. 7 | **Verified** | `duet_agent::run` catches a panic anywhere in the run and ends it as `Failed{internal error: …}`; the CLI catches errors and panics in run setup; `duet_agent::conclude` writes `summary.json` and the audit `run_end` in every case. Tests (`duet-cli/tests/termination.rs`): `a_panic_in_the_presenter_ends_the_run_as_failed_with_its_records`, `a_panic_in_the_gate_ends_the_run_as_failed_before_anything_is_sent` (summary, audit end event, chain and anchor verify). 141 live summaries: 137 completed, 4 failed; the 4 panicked runs predate the guard | Never observed live since the guard (no batch has run on it) |
| T2 | Every infrastructure failure retries in place | Operator one-shot rule; TARGET §4 | **Verified** | No attempt cap: connect, timeout, stream cut, 429 and any 5xx retry with jittered backoff capped at 60 s until the run's wall clock (one deadline shared by the loop and both providers), which ends the run as `BudgetStopped{wall_clock}`. Provider: `infrastructure_failures_retry_without_an_attempt_cap`, `a_persistent_outage_retries_until_the_deadline`, `an_attempt_in_flight_is_cut_off_at_the_deadline`, `an_interrupt_stops_retries`, `invalid_requests_are_not_retried`, `auth_errors_are_not_retried`, `backoff_grows_and_caps`. Run: `a_frontier_outage_is_retried_in_place_until_the_wall_clock_ends_the_run`, `a_local_model_outage_ends_at_the_wall_clock_without_a_degraded_result`, `errors_a_retry_cannot_fix_fail_the_run`. After the audit (branch `runtime`): a full disk (or exhausted quota or file handles) while writing the transcript, write journal, audit log, a workspace write or the summary pauses the run with one stderr notice (`disk full: waiting for space`) and retries in place with backoff until the wall clock, then `BudgetStopped{wall_clock}`; every write is all or nothing (a failed append is cut back, a failed temp file removed). Tests with an injected failing writer (`duet_fs::fault`): `a_full_disk_pauses_the_run_and_it_completes_once_space_returns`, `a_disk_that_stays_full_ends_the_run_at_the_wall_clock`, `a_failed_append_or_write_leaves_nothing_behind`, `a_shortage_is_reported_once_and_retried_until_it_clears`, `it_gives_up_at_the_deadline_or_on_an_interrupt`, `os_errors_keep_their_number_and_host_shortages_are_recognised`. The estimated usage of attempts that failed mid-stream is charged to the dollar budget, `cost_usd` and the ledger (`failed_attempt_usage`, `ledger.failed_attempts_usd`), apart from billed usage: `attempts_that_failed_mid_stream_are_charged_once`, `a_request_that_fails_in_the_end_reports_its_failed_attempts` | Never observed live. The security engine's own state (vault, local-model records) does not wait out a full disk. Failed-attempt usage of a request cut off by Ctrl-C is not charged |
| T3 | Ctrl-C finishes the current write and ends in a resumable `Failed{interrupted}` | TARGET §4 | **Verified** | `ctrl_c_ends_a_run_resumably_and_resume_completes_it` (SIGINT to the `duet` binary against a loopback scripted frontier: exit 1, summary written); `an_interrupted_run_resumes_to_completion_with_one_intact_audit_chain`; `an_interrupt_stops_retries`. The flag now also stops a frontier wait and retry waits at once. After the audit (branch `runtime`): it also kills a running command's whole process tree at once (checks included); the call is recorded as `interrupted` in the transcript and its result is not: `an_interrupt_kills_a_running_command_and_ends_the_run_resumably` (a 60 s sleep with a detached descendant, stopped within seconds, descendant gone), `a_stop_kills_the_whole_tree_at_once` (sandbox) | Never exercised live |
| T4 | `duet resume <run>` reapplies or rolls back pending writes and continues | TARGET §4 | **Verified** | `interrupted_write_is_rolled_back` (journal); `an_interrupted_run_resumes_to_completion_with_one_intact_audit_chain` (usage of both sessions, one anchored chain with two `run_end` events); `ctrl_c_ends_a_run_resumably_and_resume_completes_it` (through the binary; `audit verify` intact; a completed run and `../elsewhere` are refused, D4). A partly answered turn is dropped and re-decided | No live resume yet |
| T5 | Budgets (frontier dollars, wall clock) end in `BudgetStopped` | TARGET §4 | **Verified** | `the_dollar_budget_stops_the_run`, `the_wall_clock_stops_the_run`, `the_dollar_budget_ends_a_run_as_budget_stopped` (binary, exit 3); outages that outlast the wall clock: see T2 | The binary's wall clock is at least 1 minute (`limits.wall_clock_minutes`), so its stop is tested through the library only. Never observed live |
| T6 | The transcript is synced as it is written and repairs a torn tail | PLAN M2 | **Verified** | `private_modes_and_torn_tail_repair` | — |

### 3.7 Local backends

| ID | Requirement | Source | Verdict | Evidence | Gap / next action |
|---|---|---|---|---|---|
| L1 | oMLX | Operator requirement; TARGET §10 | **Verified** | Every live hybrid run (LAN oMLX `omlx-coding`); `local-eval`; M1 live test (PLAN §10) | Only over the plaintext LAN opt-in; the loopback path was never run live |
| L2 | Ollama | Operator requirement | **Unverified** | Preset (`presets_are_loopback_and_named_uniquely`); discovery and `/api/show` context parsing against mocked replies (`discovery_identifies_backends_by_native_endpoints`, `probes_listing_then_native_endpoints`) | `backend_smoke::ollama` has never run. Run it, plus one short hybrid task |
| L3 | LM Studio | Operator requirement | **Unverified** | Preset; `/api/v0/models` parsing (`reads_native_documents`), mocked | `backend_smoke::lmstudio` has never run |
| L4 | llama.cpp | Operator requirement | **Unverified** | Preset; `/props` parsing, mocked | `backend_smoke::llamacpp` has never run |
| L5 | vLLM | Operator requirement | **Unverified** | Preset; `max_model_len` from `/v1/models`, mocked | `backend_smoke::vllm` has never run |
| L6 | mlx_lm.server | TARGET §8 | **Unverified** | Preset | `backend_smoke::mlx` has never run |
| L7 | Constrained JSON output where the backend supports it; text tool-call recovery | TARGET §6.3; PLAN M1 fix 7 | **Partial** | `response_format` json_schema is sent (`chat.rs:69`, asserted in `protected.rs`); `recovers_one_well_formed_call`, `fails_closed_on_ambiguity`, `local_role_recovers_text_tool_calls` | Seen working only on oMLX. Whether the other servers accept `json_schema` is unknown (the smoke tests would show it) |
| L8 | Local context handling: chunking ~60K chars for a 65K window | PLAN §9 | **Verified** | `chunks_on_line_boundaries`; live L3/L4 logs digested | — |

### 3.8 Configuration, bootstrap, TUI

| ID | Requirement | Source | Verdict | Evidence | Gap / next action |
|---|---|---|---|---|---|
| CF1 | Launch with no config bootstraps: probe 127.0.0.1 preset ports; use a single unambiguous server for that run and record it; otherwise print the `duet config set` commands; never write config | Operator bootstrap requirement; TARGET §8 | **Partial** | `bootstrap_uses_a_single_server_for_this_run_only`, `bootstrap_asks_when_the_choice_is_ambiguous_or_empty`, `picks_only_unambiguous_choices` (mock servers) | Never exercised against a real local server. Combine with L2–L6 |
| CF2 | Typed registry, owner/project scopes, `duet config get/set/list/preset` | TARGET §8 | **Verified** | `every_default_is_valid`, `owner_and_project_merge_with_origins`, `set_owner_validates_and_persists`, `unknown_keys_in_files_are_errors`, `a_preset_goes_through_the_audited_loosening_path` | — |
| CF3 | Everything configurable; "a test fails on any setting read outside the registry" | TARGET §2.7, §8; PLAN §3 | **Partial** | Reading an unknown key is a runtime error (`ConfigError::Unknown`); `every_registry_key_is_on_exactly_one_screen` | No such source-scanning test exists. Several behaviour constants are hard-coded and not configurable: `MAX_TEXT_ONLY_TURNS`, `MAX_LENGTH_STOPS` (`run.rs`), the provider backoff cap (60 s, `retry.rs`) and timeouts (`client.rs`), `ask_local` ≤ 6 questions, `read_raw` ≤ 500 lines |
| CF4 | The TUI covers every setting and shows its origin; loosening shows a diff, needs `y` and is audited; the project scope refuses owner-only keys | TARGET §8; PLAN M6 | **Verified** | `every_registry_key_is_on_exactly_one_screen`, `settings_screens_show_every_value_and_its_origin`, `loosening_shows_the_diff_and_cancel_leaves_everything_unchanged`, `confirmed_loosening_applies_and_records_what_it_weakened`, `project_scope_refuses_owner_only_keys_and_loosening`, `tightening_applies_directly_and_is_audited`, `edits_are_validated_and_cancellable` | — |
| CF5 | TUI Audit, Run, Sensitivity-tester and Models screens | TARGET §8 | **Verified** | `audit_screen_lists_records_and_verifies`, `run_screen_follows_the_transcript`, `sensitivity_tester_explains_matches`, `models_screen_runs_doctor_offline_and_online_on_request`, `empty_workspace_screens_render`; after the audit, `a_terminal_without_a_usable_size_is_refused_with_the_minimum` (no terminal, 0x0 or under 80x24: a message and exit 1, checked by hand in a pseudo-terminal too) | Tested with `TestBackend` only, never used in a real terminal beyond that size check |
| CF6 | TUI extras: backend auto-detection, connection/cache/prefill tests, custom detector patterns, purge from Data, starting runs | TARGET §8; PLAN M6 "Open" | **Not implemented** | — | M6 |
| CF7 | `duet doctor`: offline by default, `--online` without model calls, `--json`, exit code = worst result, key never printed | TARGET §8 | **Verified** | `offline_doctor_uses_no_network_and_never_prints_the_key`, `online_doctor_lists_models_and_context_without_model_calls`, `doctor_shows_the_approval_mode_and_notes_release_keys_in_a_development_build`, `release_keys_warn_only_in_a_release_build` | — (after the audit: the release-keys check is SKIP with a note in a development build and warns only in a build made by `tools/release.sh`, which sets `DUET_RELEASE_BUILD`) |
| CF8 | Doctor cache-reuse check and update check; frontier presets (z.ai, Anthropic, OpenAI) | TARGET §8; PLAN M6 | **Not implemented** | — | M6; the update check needs a release channel (SC5) |
| CF9 | Anthropic Messages and Responses dialects | TARGET §3.1, §10 | **Not implemented** | Moved to M6 (PLAN §10 scope changes) | — |

### 3.9 Security defaults

| ID | Requirement | Source | Verdict | Evidence | Gap / next action |
|---|---|---|---|---|---|
| SD1 | Hybrid is the default; passthrough needs `--no-privacy` and prints a banner | SECURITY Secure defaults | **Verified** | `passthrough_needs_an_explicit_acknowledgement` | — |
| SD2 | Loosening needs `--confirm`, prints the diff and is recorded in a hash-chained config audit | SECURITY; SbD-1 | **Verified** | `loosening_a_setting_needs_confirm_and_is_audited`, `loosening_needs_confirmation_and_tightening_does_not`, `proposals_match_what_apply_enforces` | — |
| SD3 | Project config can only tighten and cannot set owner-only keys | SECURITY; ARCH §12 inv. 4 | **Verified** | `project_cannot_loosen`, `project_cannot_set_owner_only_keys`, `project_may_add_globs`, `approval_is_owner_only_and_turning_it_down_needs_confirmation` | — |
| SD4 | Sandbox on; commands' network only through the egress proxy to the listed package registries (none for `sensitive_data` commands and checks that can read protected source), environment cleared to an allowlist | SECURITY (Command network) | **Verified** | macOS (gate) and Linux: `network_is_denied_by_default`, `unix_sockets_are_unreachable_without_network`, `with_the_proxy_route_a_command_reaches_the_proxy_and_nothing_else`, `with_the_proxy_route_a_command_reaches_the_servers_it_starts`, `unix_sockets_stay_unreachable_with_the_proxy_route`, `the_bridge_helper_hands_connections_to_the_host`, `credential_stores_in_the_home_directory_are_unreadable`, `ordinary_commands_reach_listed_registries_only_in_both_modes`, `sensitive_values_never_reach_a_registry_or_the_frontier`, `checks_that_read_protected_source_have_no_network`, `a_hybrid_run_reaches_the_registry_and_is_told_what_was_refused` (duet itself as the bubblewrap bridge helper), the proxy's own tests (`duet-egress`), `environment_is_cleared_to_the_allowlist`, `writes_inside_workspace_and_scratch_only`, `timeout_kills_detached_descendants`, `a_stop_kills_the_whole_tree_at_once`, `sandbox_is_detected`. Linux run (`tools/linux-check.sh`, 2026-09-26, same image, bubblewrap 0.8.0, rustc 1.98.1, kernel 7.0.14-orbstack): every test passing privileged, unprivileged and as root (clippy stopped on two pre-existing `chunks_exact_to_as_chunks` lints in `duet-boundary` with rustc 1.98). Live (macOS, default settings): `cargo add` + build and `npm install` through the proxy, another host refused (`live_registries`, and a `duet run`); a sandbox that cannot start refuses the run (`real_bubblewrap_without_namespaces_refuses`) | Linux not in the gate (see P7). The Linux seccomp filter covers x86-64 and AArch64; elsewhere commands without unrestricted network are refused. Seatbelt limits (development ports only, positive list): SECURITY "Command network" |
| SD5 | Tools and commands cannot write `.git` or `.duet` at any depth; reads are handle-relative and refuse symlinks | SECURITY "Still prevented"; ARCH §12 inv. 8 | **Verified** | `reserved_directories_are_read_only_at_any_depth`, `reserved_directories_at_any_depth`, `refuses_symlinks_on_the_path_and_at_the_leaf`, `normalizes_and_rejects_escapes`, `every_duet_path_in_the_sources_is_registered` | For commands: macOS in the gate, Linux via `tools/linux-check.sh` (P7); on Linux a `.git`/`.duet` a command creates is removed after it ends |
| SD6 | A single hardened git helper (no hooks, no fsmonitor, no global config) | PLAN M2 | **Verified** | `hostile_repository_config_cannot_run_hooks_or_fsmonitor` | — |
| SD7 | Operator approval of risky actions (`off`/`risky`/`all`); fail closed without a terminal; decisions audited without content | SbD-3; SECURITY Oversight | **Verified** | `classification_by_mode`, `source_and_test_files_are_defined_conservatively`, `decisions_are_asked_enforced_and_audited_without_content`, `with_approval_on_a_run_without_a_terminal_is_refused_before_it_starts`, `a_denied_write_is_a_tool_error_and_the_report_counts_what_was_withheld`, `only_an_explicit_yes_approves` | — |
| SD8 | Memory-safe Rust, `unsafe_code = "forbid"` | PLAN SbD | **Verified** | Workspace lint (`Cargo.toml:50`); clippy `-D warnings` in the gate | — |
| SD9 | Credentials are referenced only by environment-variable name; values are never printed | SECURITY; PLAN SbD | **Verified** | `offline_doctor_uses_no_network_and_never_prints_the_key`, `missing_prerequisites_fail_without_revealing_values` | — |
| SD10 | `SECURITY.md` is complete: a disclosure contact and `security.txt` | SbD gate | **Not met** | Contact is `security@<domain>` with a `TODO(operator)`; `docs/security.txt` has placeholders | The operator sets the address and Expires |
| SD11 | The frontier provider's data-retention terms are documented in the threat model | PLAN SbD-3 | **Not met** | SECURITY only assumes an "honest-but-curious recipient"; z.ai terms are not stated | Add them |
| SD12 | Every disclosure path found is fixed at the class level, has a regression test, and is published as an advisory with its CWE | SECURITY Advisories | **Verified** | DUET-2026-001…008 published. Regression tests: 001 `the_models_own_messages_are_sanitized_too`; 002/003 `surnames_are_replaced_alone_unless_the_word_is_public`, `numbers_are_replaced_in_their_other_spellings`; 004 `commands_cannot_read_sensitive_files_unless_their_output_stays_local`; 005 `copied_runs_split_by_a_public_window_are_merged`, `overlap_never_panics`; 006 `commands_cannot_read_git_history_or_run_state`; 007 `values_escaped_in_tool_arguments_are_replaced_and_checked`; 008 `a_value_inside_a_longer_detected_span_is_known_alone` | — |

The claims under SECURITY.md "Still prevented", mapped to the rows above:

| Claim | Row(s) | Verdict |
|---|---|---|
| Commands cannot read sensitive paths, protected source, `.git` or `.duet`, in any encoding | P7, IP3 | Verified (macOS in the gate; Linux via `tools/linux-check.sh`) |
| Commands reach only the listed package registries, through an audited proxy (`sensitive_data` commands nothing); a project cannot loosen it | SD4, SD3 | Verified (macOS in the gate; Linux via `tools/linux-check.sh`) |
| `.git` and `.duet` are not writable from tools or commands | SD5 | Verified (macOS; Linux via `tools/linux-check.sh`) |
| No known value, detected secret, personal datum or copied span reaches the frontier; failing requests are blocked | P1, P4 | Verified |
| A resolved secret is written only to secret sinks | P8 | Verified |
| Policy cannot be loosened by repository content | SD3 | Verified |
| Denials, sensitive commands and blocked sends are in the anchored audit log | P16 | Verified (D3 fixed after the audit) |

### 3.10 Supply chain

| ID | Requirement | Source | Verdict | Evidence | Gap / next action |
|---|---|---|---|---|---|
| SC1 | `cargo deny`: advisories, licenses (a GPL-compatible allowlist), bans, sources (crates.io only) | SECURITY Supply chain | **Verified** | This gate run: all four ok | — |
| SC2 | Builds use the committed `Cargo.lock` (`--locked`) | SECURITY | **Verified** | `tools/gate.sh` clippy and test with `--locked` | — |
| SC3 | CycloneDX SBOM, generated offline by Duet's own tool | PLAN SbD-3 | **Verified** | `components_are_the_runtime_and_build_closure_with_licenses_and_hashes`, `the_document_is_reproducible_and_the_serial_is_a_uuid`, `the_sbom_of_duet_cli_lists_its_dependencies_with_licenses` | — |
| SC4 | Signed releases (`ssh-keygen -Y`), verification script, the key never in the repo | PLAN SbD-3 | **Verified** | `verify_release_accepts_a_signed_release_and_rejects_tampering`, `release_refuses_to_start_without_an_operator_key_or_the_right_version` (throwaway key) | Never used for a real release (deliberately: after acceptance, PLAN §3 2026-09-24) |
| SC5 | Published release channel, outdated-version check, advisory feed | PLAN SbD-3 | **Not implemented** | Stated as "Not yet" in SECURITY | — |
| SC6 | Build reproducibility verified independently | SECURITY Supply chain | **Not implemented** | Stated as not verified | — |
| SC7 | Every commit passes `tools/gate.sh`; a pre-push hook runs it (no hosted CI) | PLAN §2.6, SbD-2 | **Partial** | `tools/install-hooks.sh` exists; gate passes at HEAD | Hooks are **not installed** in this clone (`.git/hooks` holds only samples) and no remote is configured, so nothing enforces the gate per commit. Install the pre-push hook (operator). After the audit, README "Development" makes installing them the first step |

### 3.11 Evaluation harness

| ID | Requirement | Source | Verdict | Evidence | Gap / next action |
|---|---|---|---|---|---|
| EV1 | `duet-eval selftest`: the proxy catches a planted canary; statistics reproduce known intervals; prices verified | PLAN M0 acceptance | **Verified** | Run in this audit: "selftest ok". The judge schema is not part of `selftest`, but is unit-tested (`parses_tool_use_response`, `json_answers_parse_with_or_without_a_fence`, `a_malformed_answer_is_retried_once`) | — |
| EV2 | Every task package validates and its seal verifies; hidden tests never enter the workspace | PLAN M0; DOGFOOD_SUITE §10 | **Verified** | `duet-eval validate` (this audit): 11/11 ok. `loads_validates_and_seals`, `renders_assets_over_starter_and_keeps_holdout_out` | — |
| EV3 | Sealed grading: hidden tests (several commands allowed), secret-sink scan | PLAN M0.2 | **Verified** | `grades_a_rust_task_end_to_end`, `a_hidden_binary_that_does_not_compile_fails_only_its_own_tests`, `parses_libtest`, `parses_tap_top_level_only` | — |
| EV4 | Statistics: paired bootstrap, non-inferiority, exact binomial leak bound | PLAN M0.2 | **Verified** | `stats.rs` (8), `quality_is_decided_by_the_judge_and_the_task_majority`, `a_single_leak_fails_privacy` | — |
| EV5 | Judge: rubric, agent names scrubbed, canaries redacted, two repeats | PLAN M0.2 | **Verified** | `scrubs_agent_names_and_canaries`, `diffs_directories_and_drops_build_output`; `gate1-flash`, `gate2` judged | — |
| EV6 | Lanes `claude-code` and `codex` run on the operator's CLI subscriptions through the proxy; one smoke run per lane before M5 | PLAN §3 2026-09-24 | **Partial** | `subscription_lanes_use_no_api_key_and_route_through_the_proxy`, `every_frontier_lane_routes_through_the_proxy`, `a_copied_main_login_is_refused` | No batch has ever run either lane: the smoke runs are pending |
| EV7 | Lane `pi-glm` (external reference) | PLAN M0.4 | **Verified** | `pilot-pi-flash`, `gate1-flash` | — |
| EV8 | Lane `duet-local-only` (floor reference) | PLAN M0.2 | **Unverified** | Lane defined; the CLI has `--mode local-only` | Never run |
| EV9 | Runs decided by the host or infrastructure are invalid and retried | PLAN §10 M2; `24d921d` | **Verified** | `classifies_infrastructure_outcomes`; `l2l4` disk-full run set aside | — |
| EV10 | The harness counts only real terminal states and separates product failures from infrastructure | Operator one-shot / orthogonal-outcomes rule | **Verified** | `run.json` records Duet's `terminal`; a Duet run with no terminal state (crash, or killed at the time limit) or stopped by its own gate is a `product_failure`: valid, pass rate 0, no success, judge score 0. Host failures, a failed launch and provider refusals before any decision stay invalid. The rules also apply when a batch is loaded, so stored batches are reclassified without being rewritten. Tests: `a_duet_run_without_a_terminal_state_is_a_failure_of_the_lane`, `a_run_stopped_by_duets_own_gate_is_a_product_failure_not_infrastructure`, `genuine_infrastructure_stays_invalid`, `completed_and_external_runs_are_unchanged_and_the_rules_are_idempotent`, `reads_the_terminal_state`. Read-only scan of `results/`: 9 run directories change (§4.4 D1, D2) | Gate 2 (PLAN §10) counted `gate2/M1-duet-hybrid-s1` and `S1-duet-hybrid-s2` as passing; under these rules they are failures of the hybrid lane, and Gate 2 should be recomputed |

### 3.12 Novelty and provenance

| ID | Requirement | Source | Verdict | Evidence | Gap / next action |
|---|---|---|---|---|---|
| N1 | No names, formats or code from other coding agents in `crates/` or `fuzz/` (except the eval lane adapters) | PLAN §2.3; ARCH §12 inv. 9 | **Verified** | Gate "provenance" step passed | — |
| N2 | No prompts, tool schemas or design documents from other agents; each ported v1 file checked before porting | PLAN §2.3; operator | **Partial** | The grep covers names and one patch-format marker only | Prompt and design novelty cannot be checked mechanically, and no per-file provenance record for the v1 ports exists. Record one (file, v1 source, check done) |
| N3 | Rust only; no Python anywhere, including the harness | PLAN §2.4 | **Verified** | `git ls-files '*.py'` is empty; the harness is `duet-evals` in Rust. Python appears only as a skeleton language | — |
| N4 | External agents only as black-box evaluation lanes | PLAN §3 2026-09-23 | **Verified** | Adapters only launch programs (`crates/duet-evals/src/lanes/`), the only allowlisted path in the provenance step | — |
| N5 | GPL-3.0-or-later with SPDX headers | PLAN §2.5 | **Verified** | `LICENSE`; the gate's "license headers" step; cargo-deny licenses | — |

### 3.13 Large repositories

| ID | Requirement | Source | Verdict | Evidence | Gap / next action |
|---|---|---|---|---|---|
| LR1 | Large repositories must work, not only small scopes | Operator requirement; PLAN M6 | **Not met** | The largest measured task is L3 (70 files, 8.4K code lines); the others have 3–23 files. X1/X2 (real open-source repositories) are not built | Build X1/X2 and run them in hybrid |
| LR2 | File listing from `git ls-files` with no file-count cap | PLAN §6 | **Partial** | `lists_files_without_reserved_or_ignored` | No test asserts the absence of a cap at scale |
| LR3 | Repository map; search scaling | PLAN M6 | **Not implemented** | — | Also unmeasured: `hidden_from_commands` walks the whole working tree before every command (`engine.rs:905`) |
| LR4 | Bulky offload and masking keep context bounded on larger code | PLAN M4 | **Verified** | `bulky.rs` tests; live on L3 (`gate3a`/`gate3b` ledgers: bulky 5–162K tokens per run) | — |

## 4. Summary

### 4.1 Counts by area

| Area | Verified | Partial | Unverified | Not met | Not implemented | Total |
|---|---|---|---|---|---|---|
| Quality (Q) | 1 | 5 | 0 | 1 | 0 | 7 |
| Agent loop and context (A) | 5 | 3 | 0 | 0 | 0 | 8 |
| Privacy (P) | 14 | 1 | 1 | 0 | 1 | 17 |
| IP (IP) | 5 | 0 | 0 | 0 | 0 | 5 |
| Cost (C) | 3 | 2 | 0 | 1 | 0 | 6 |
| Termination / resume (T) | 6 | 0 | 0 | 0 | 0 | 6 |
| Local backends (L) | 2 | 1 | 5 | 0 | 0 | 8 |
| Config / bootstrap / TUI (CF) | 4 | 2 | 0 | 0 | 3 | 9 |
| Security defaults (SD) | 10 | 0 | 0 | 2 | 0 | 12 |
| Supply chain (SC) | 4 | 1 | 0 | 0 | 2 | 7 |
| Evaluation harness (EV) | 8 | 1 | 1 | 0 | 0 | 10 |
| Novelty / provenance (N) | 4 | 1 | 0 | 0 | 0 | 5 |
| Large repositories (LR) | 1 | 1 | 0 | 1 | 1 | 4 |
| **Total** | **67** | **18** | **7** | **5** | **7** | **104** |

### 4.2 Release blockers

In this audit's judgement, these must be Verified, or explicitly re-scoped by the operator, before a
release:

1. **Quality on the release build (Q1, Q3, EV6).** The last judged non-inferiority result is Gate 2
   on `a5346f4`, with one judge. Everything since (bulky offload, IP levels, name fixes, oversight)
   changes what the frontier sees, and `gate3b` was behind on pass rate on 3 of 4 tasks. The product
   claim is "frontier-level results". It needs an M5 run on the release build with both judges, and
   one smoke run each for `claude-code` and `codex`.
2. **Termination guarantee (T1, T3, T4, T5, EV10).** Live runs have panicked without a terminal
   state, and the harness counted them as valid. `BudgetStopped`, Ctrl-C → `Failed{interrupted}`
   and `duet resume` have no tests and were never exercised. The one-shot requirement is the
   operator's. Needed: a panic guard, tests for each terminal path, and a harness that records the
   terminal state. *Resolved on branch `term`:* all three, with tests (T1–T5, EV10 Verified); still
   to be observed in a live batch.
3. **Retry-in-place policy (T2).** Bounded retries end a run as `Failed` after about 15 minutes of
   outage. That run can be resumed, but it contradicts "every infrastructure failure retries in
   place". Decide, then test. *Resolved on branch `term`:* no attempt cap; the run's wall clock and
   dollar budget are the only stop (`BudgetStopped`), tested with scripted transports (T2).
4. **Local backends (L2–L6, CF1, L7).** The docs claim support for Ollama, LM Studio, llama.cpp,
   vLLM and mlx_lm.server. None has ever answered a real request. Run `backend_smoke` for each, and
   the no-config bootstrap against one real loopback server, or remove the claims.
5. **Linux sandbox (P7, SD4).** Duet's main privacy mechanism for derived data (DUET-2026-004/006) is
   OS access control. On Linux it has never been executed. Verify on Linux, or declare macOS-only.
   *Resolved on branch `linux`:* the sandbox and agent tests run on Linux in Docker
   (`tools/linux-check.sh`) and pass privileged, unprivileged and as root; without namespaces
   commands are refused. Defects found on the way were fixed (P7).
6. **Red-team pass and fuzzing (P11, P12).** Both are required by the SbD gate and by SECURITY.md
   ("run before releases"). Neither has happened.
7. **Security disclosure policy (SD10, SD11).** Placeholder contact, placeholder `security.txt`, and
   no provider data-retention terms.
8. **Honest documentation (C2, D6).** README still lists "Cheaper" as a goal. It also describes a
   local brief that is off by default. The "~1.4×" premium is below the 1.5–2.4× measured per batch.
   Secure by Design requires the limits to be stated accurately.
   *Resolved after the audit (branch `nits`), for the items listed in D6.*
9. **Large repositories (LR1).** An operator requirement with no evidence beyond 8.4K lines.

**Status update 2026-09-25** (after the audit; evidence in PLAN §10):

| # | Blocker | Status |
|---|---|---|
| 1 | Quality on the release build | **Verified on `d55d2cc`** (Gate 2 re-established, `results/gate2d`): both judges, hybrid 20.8 vs 20.2 /30 (Δ +0.6, lower bound +0.0), hidden 97.2% vs 95.9%, 0 leaks in 18/18. Two defects found on the way were fixed: DUET-2026-009 (local answer quoting data, `aa6226c`) and a false-positive block on placeholder values (`d55d2cc`). Still open: M5-scale run including L2–L5 and the `claude-code`/`codex` smoke runs (Claude token pending). |
| 2 | Termination guarantee | **Verified**: merged (`359930e`) with tests; observed live — every run in `gate2b`/`gate2c`/`gate2d` ended in a terminal state; hands-on Ctrl-C → `duet resume` → Completed on a fresh project. |
| 3 | Retry in place | **Verified by tests** (`359930e`); not yet observed during a real outage. |
| 4 | Local backends | **Re-scoped by operator decision (2026-09-24):** live acceptance is the configured remote oMLX server (verified live throughout); other backends are verified against mock servers only and documented as such. |
| 5 | Linux sandbox | **Verified in Docker** (branch `linux`, P7/SD4): privileged, unprivileged user namespaces and root all pass; no namespaces → refused. Not in the gate; not yet run on an AppArmor-restricted host. |
| 6 | Red-team pass and fuzzing | Fuzzing ran (SbD-2: 60 s per target, 7.5M executions, 0 failures) and property tests are in the gate. Red-team pass parked until last by operator decision. |
| 7 | Disclosure policy | Contact placeholder kept by operator decision; provider data-retention terms still to document. |
| 8 | Honest documentation | Resolved (`faf3098`). |
| 9 | Large repositories | Open (operator decision pending on X1/X2). |

### 4.3 Known limits (documented; acceptable for a first release if stated)

- Cost is a privacy premium, not a saving (C1 superseded by decision). Report the ratio with its
  interval.
- Residual leakage by design (TARGET §6.6, SECURITY "What is not protected"): existence and shape,
  paraphrase in local answers, the task text, skeletons, Open source code.
- Protected code can be extracted a few bytes per run by a deliberately adversarial frontier
  (SECURITY, IP limits).
- Value filters do not cover values under 4 bytes, first names alone, other casings or re-encodings.
  Access control covers re-encoding (P7).
- Derived files in `target/` and `node_modules/` are not tracked as sensitive.
- The operator's LAN local model is reached over plain HTTP by explicit opt-in (P9).
- No automatic retention deletion (P14); no Anthropic or Responses dialects (CF9); TUI extras (CF6);
  doctor update and cache checks (CF8); no release channel or reproducibility check (SC5, SC6).
- Calibration band not met on easy tasks (Q6); electricity is an estimate (C3).

### 4.4 Defects found in this audit (none fixed here)

| ID | Defect | Evidence | Suggested fix |
|---|---|---|---|
| D1 | The harness grades runs that crashed (exit 101, no terminal state) as normal runs | `gate2/M1-duet-hybrid-s1` and `gate2/S1-duet-hybrid-s2` (panic at `overlap.rs:105`) graded 100% and counted in Gate 2, which PLAN §10 records as "all runs valid". The privacy result is unaffected (the crash was fail-closed, before sending) | Record `summary.json` `terminal` in `run.json`; a run without one fails. **Fixed** after the audit (branch `term`): see EV10 |
| D2 | A product failure is excluded as infrastructure | `gate3c/S2-duet-hybrid-s2/-s3`: Duet's gate blocked the first send (a false-positive name), and the harness marked the runs invalid ("no frontier request") | Classify by the agent's terminal state before the proxy statuses. **Fixed** after the audit (branch `term`): see EV10 |
| D3 | The audit anchor is keyed by the workspace path, so moving the workspace makes `duet audit verify` fail with "no anchor" (exit 2) | `l2l4/L2-duet-hybrid-s1` after the move to EXT_DISK; the anchor sits under a different path hash | Key anchors by run id plus the chain's genesis, or search the anchors by run id. **Fixed** after the audit (branch `nits`): keyed by run id and first-record hash, earlier layout still read; see P16 |
| D4 | `duet resume <run_id>` does not validate the id (the `audit` commands use `checked_run_id`) and does not refuse a run that already ended | `duet-cli/src/main.rs` `Cmd::Resume`; `run.rs` ignores `Entry::End` on resume | Validate the id; refuse or confirm when the transcript has an `End` entry. **Fixed** after the audit (branch `term`): the id is checked and a completed run is refused; see T4 |
| D5 | Electricity is computed from wall seconds, not local busy seconds | `duet-evals/src/lanes/mod.rs` (`electricity(cfg.local_watts, record.wall_seconds)`) versus PLAN M0.2 | Use the ledger's local busy seconds |
| D6 | Documentation claims ahead of the evidence | README "Goals" still lists **Cheaper**. README says the local model briefs the frontier at run start (default off). README, TARGET and PLAN state a "~1.4×" premium (batches: 1.5–2.4×). ARCH §12 says every invariant is test-backed (inv. 7 is not). TARGET §8 promises a registry-read test that does not exist. The PLAN header ("Gate 3 of M4; SbD-2 next") and TARGET header ("implemented through M4.5 and SbD-1") are stale | A docs-only pass. **Fixed** after the audit (branch `nits`): all listed items corrected, except the PLAN §3 decision row, which keeps its original text |

Fixed on branch `term` (2026-09-24): D1 and D2 (the harness records Duet's terminal state and
classifies by it, EV10) and D4 (`duet resume` checks the id and refuses a completed run, T4). A
read-only scan of `results/` with the new rules changes nine run directories: no terminal state
(exit 101), now product failures instead of valid runs: `gate2/M1-duet-hybrid-s1` and
`gate2/S1-duet-hybrid-s2` (graded 100%; the same runs in `m3-hybrid-v2`), `gate3a/L3-duet-hybrid-s3`
(20%) and `m3-hybrid/M3-duet-hybrid-s3` (20%); stopped by Duet's gate, now product failures:
`gate3a/S1-duet-hybrid-s2` (was valid, graded 100%) and `gate3c/S2-duet-hybrid-s2`/`-s3` (were
invalid). No other batch changes; no infrastructure-invalid run became valid.

## 5. What would move this to "accept"

In order: fix D1/D2 and add the termination tests (blocker 2). Run `backend_smoke` on each backend
and the sandbox tests on Linux (4, 5). Run fuzzing and the red-team pass (6). Fill in the disclosure
details (7) and correct the documents (8). Then run M5 on the release build with both judges,
including L2 and at least one X-tier repository (1, 9). Re-audit against this document after that.
