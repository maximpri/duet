# Duet's internal evidence: did the frontier-planner / local-executor split meet "frontier-level results at a fraction of the cost"?

Scope: only the project's own local files (repo `/Users/maximp/OpenCode/duet_agent`, its gitignored `notes/`, `HANDOFF-GOAL.md`, `docs/`, `evals/`, plus the project memory directory). No web sources. All paths below are absolute file paths; line numbers are from `cat -n` / `awk NR` reads on 2026-09-22 (HEAD `5f83b89`, with 19 uncommitted files in the working tree).

Reading conventions:
- **Naming anonymization.** `HANDOFF-GOAL.md`, `notes/host-review-2026-08-24.md` and `notes/findings-2026-08-22.md` refer to "local baseline", "frontier route" and "event-log approach". The memory file quotes the same operator directive as "pi, codex, opencode" ([memory/duet-host-floor-requirement.md](/Users/maximp/.claude/projects/-Users-maximp-OpenCode-duet-agent/memory/duet-host-floor-requirement.md) vs [HANDOFF-GOAL.md:259-260](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)). The dogfood reference table names the local-only arkanoid reference "Pi + omlx-coding (local only)", saved as `pi-local-qwen.html` ([references.csv row ref-pi-local](/Users/maximp/OpenCode/duet_agent/evals/dogfood/results/2026-09-08-roadmap-qualification-dcb11af3cd69/report/references.csv)). So "local baseline" means Pi running the local qwen model (`omlx-coding`). "frontier route" means Codex, and "event-log approach" means OpenCode.
- **Moved paths.** `HANDOFF-GOAL.md` cites `docs/findings-2026-08-22.md` and `docs/host-review-2026-08-24.md`. Those files now live in `notes/`, because dated notes were moved out of the repository in the 08-28 docs cleanup ([CHANGELOG.md:620-621](/Users/maximp/OpenCode/duet_agent/CHANGELOG.md)).
- **Missing artifacts.** The arkanoid baseline artifacts (`~/OpenCode/arkanoid_baselines/`) do **not exist** on disk now (`ls` failed), so no score could be re-checked against its artifact.

---

## Q1. Quality results: blind arkanoid judgings (duet vs frontier-only vs local-only), the controlled same-model experiment, dimensions won or lost, and judge drift

### Takeaway
In every recorded blind arkanoid judging, duet scored below both references. The judgings ran 2026-08-19 to 08-26: 7 rounds, N=1 artifact per lane per round, 50-point rubric of five 10-point axes. Duet's best routed score was 39/50 on 08-25 at about 95% local. In the same judging the frontier-only reference scored 48 and the local-only Pi + qwen reference scored 41. On 08-26 duet scored 38.5 against 47.5 and 42.5. By the end, duet won on correctness and lost on presentation, ambition and features. The operator's bar was ±1 of frontier, but the judge drifted about 5–8 points on unchanged reference files, so that bar sat below the instrument's noise floor. A controlled experiment held the local model constant across two harnesses: Pi scored 43 and duet 18. The project concluded that the host, not the model, was the variable.

### Cited Findings

**Goal definition and bar**
- Goal sentence: a routed goal must match frontier-only quality at ≥80% local routing and a fraction of the frontier-only price. Quality is blind-judged, within ±1 on the 50-point rubric, per the 2026-08-26 operator restatement. — [notes/REVIEW_2026-08-27.md:12-14](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)
- 2026-08-26 operator redefinition, verbatim: *"the goal should be meeting GLM result … so meeting local 40 is not enough. duet wont have an edge and purpose."* Success became "blind-judged parity with the frontier-only reference in the same judging (glm: 47.5–48 in the current instrument, ±1) at ≥80% local routing". Pi parity (about 41) was relabelled "only the floor". — [HANDOFF-GOAL.md:95-106](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
- Earlier bar versions:
  - 2026-08-22: exit criterion "≥ 40 blind with frontier share under 10%". — [notes/findings-2026-08-22.md:232](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md)
  - 2026-08-22 plan: "50/50 blind, or ≥43 with frontier share under 10%". — [HANDOFF-GOAL.md:557](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
  - 2026-08-24: target band 41–44, meaning "glm and/or gpt luna level". — [HANDOFF-GOAL.md:2087-2090](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
- Benchmark task: build a complete Arkanoid/Breakout game as one self-contained HTML file from a fixed 3,281-character spec. It is judged blind by an independent reviewer who reads each file in full and drives it in a real browser. — [notes/findings-2026-08-22.md:7-9](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md)
- Judging protocol on 08-25: anonymized copies, a fresh judge, full source read plus instrumented browser drive per artifact. — [HANDOFF-GOAL.md:169-172](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)

**Blind three-way judgings by date.** Rubric: Features / Code / Correctness / Polish / Perf, each out of 10.

| Date | Duet artifact / condition | Frontier-only (glm) | Local-only (Pi + qwen) | Duet | Source |
|---|---|---|---|---|---|
| 08-19 | Goal 310aeb88cb45, "NEON BREAKER" (1,801 lines) vs glm-5.2 solo (1,485 lines); head-to-head, about a 10-point scale | ~8.5 | — | ~7 | [arkanoid-frontier-parity-judge-2026-08-19.md:6-12](/Users/maximp/OpenCode/duet_agent/notes/arkanoid-frontier-parity-judge-2026-08-19.md) |
| 08-21 | Three-way comparison, same spec; duet ranks last | 9/8/8/9/7 = 41 | 8/7/7/8/8 = 38 | 5/4/4/4/5 = 22 | [HANDOFF-GOAL.md:1724-1737](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md) |
| 08-21 | Blind re-judge; duet artifact did not work (unsized canvas, undefined `LEVELS`) | 9/8/8/9/6 = 40 | 8/8/7/8/8 = 39 | 3/2/1/2/3 = 11 | [HANDOFF-GOAL.md:1608-1626](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md) |
| 08-21 | Re-judge after repair (39,855-byte artifact) | 41 | 39 | 22 ("from 11/50 to 22/50") | [HANDOFF-GOAL.md:1488-1502](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md) |
| 08-22 | Duet declared COMPLETE (scored 22 while the goal was active, 20 once it declared completed) | 43 | 40 | 5/4/3/3/5 = 20 | [HANDOFF-GOAL.md:566-580](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md) |
| 08-22 | "Phase 0" (verification fixes) | 42 | 40 | 3/3/5/3/5 = 19 | [HANDOFF-GOAL.md:401-413](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md) |
| 08-22 | "Phase 3" (defragmentation / throughput) | 43 | 41 | 6/4/3/6/5 = 24 | [HANDOFF-GOAL.md:328-337](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md) |
| 08-24 | Current version with steering on (goal 32f571eec692); "not scoreable": a scaffold with no bricks after ~5 h | codex + gpt-5.6-luna xhigh ~42–44 | — | none | [HANDOFF-GOAL.md:2407-2419](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md), [HANDOFF-GOAL.md:2082](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md) |
| 08-25 | Goal 580014b93e84, frozen `duet-m3-frozen.html` (md5 c6f18c50…), 95% local | 10/9/9/10/10 = **48** | 9/7/7/8/10 = **41** | 8/7/9/6/9 = **39** | [HANDOFF-GOAL.md:167-185](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md) |
| 08-26 | Second judging of `duet-m3b-frozen.html` (md5 54aa29ac…, after three correctness repairs) | 47.5 | 42.5 | **38.5** | [HANDOFF-GOAL.md:121-131](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md) |
| 08-26 | m4 (goal 65ce967832fe) | — | — | "32" was a mid-rework snapshot, "not a result"; m4 was "unjudged at freeze" | [followups-2026-08-26.md:40-42](/Users/maximp/OpenCode/duet_agent/notes/followups-2026-08-26.md); [references.csv](/Users/maximp/OpenCode/duet_agent/evals/dogfood/results/2026-09-08-roadmap-qualification-dcb11af3cd69/report/references.csv) |

- Summary of these judgings in the 08-27 review: "1 artifact/lane/round, 7 rounds; duet 22→20→19→24→30→39→38.5; frontier refs 41–48; local-only 39–42.5". The review calls this "the only measurement of the goal; N=1 per cell". — [notes/REVIEW_2026-08-27.md:91](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)
- 08-27 verdict: "Best blind score 39/50 (2026-08-25, N=1), one point under the local-only single-loop baseline (40–42) and 6–9 under the frontier-only references (45–48 in the same judging)." The review adds: "The routed lane has never been compared to duet's own frontier-only configuration on the artifact task; the 'frontier reference' is three different agents." — [notes/REVIEW_2026-08-27.md:23-28](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)
- No arkanoid or tetris judging after 08-26 appears in any file read. In the dogfood reference table, the m4 artifact stays "unjudged at freeze", and the codex + luna reference has no per-axis scores ("42-44 (measured 2026-08-24)"). — [references.csv](/Users/maximp/OpenCode/duet_agent/evals/dogfood/results/2026-09-08-roadmap-qualification-dcb11af3cd69/report/references.csv)

**The "30 best measured" label conflicts across files**
- `HANDOFF-GOAL.md:2085` and `notes/host-review-2026-08-24.md:23` list "duet, best measured | frontier + local | 30".
- The primary source, [notes/findings-2026-08-22.md:891-906](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md), lists the 30 under "duet local-only build … all changes enabled together". That series is the same-model experiment, in which the local model also ran duet's orchestrator role ([findings:256-260, 319-321](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md)). The 08-27 review also puts 30 in the routed trajectory ([REVIEW_2026-08-27.md:91](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)).

**Controlled experiment: same local model (`omlx-coding`), two harnesses** ([notes/findings-2026-08-22.md](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md))
- §8, lines 256-267: Pi harness 9/9/8/9/8 = **43**; duet harness 4/3/1/5/5 = **18**. The file calls this "a 25-point swing attributable to the host alone".
  - Duet's build could not be played for one second: three lives drained in three frames.
  - Duet had still declared it `completed` with zero gaps (lines 269-289).
  - Caveat in the file: duet's orchestrator prompts were written for a frontier model but run by a local one, so some planning quality reflects prompt/model fit (lines 319-324).
- §10, lines 442-454: Pi **45**; duet with turn windows 8/14 **25**; duet baseline with windows 3/2 **16**. Raising the no-progress windows was worth +9, "the largest single lever found".
- §12, lines 577-594: `require_observed_work` gave 25 vs Pi 43. The gate never fired.
- §13, lines 647-662: "verify like a player" gave **19**. Pi on the same model scored "41-45, every time".
- §17, lines 891-906: all changes combined gave **30** (7/7/4/6/6) vs Pi 42 (9/8/8/9/8). Duet's code grew 98 → 630 → 1,110 → 1,473 lines across these runs.
- The file also cautions: "Solo A/B on a stochastic pipeline at n=1 is weaker evidence than I treated it as." — lines 943-946
- Summaries elsewhere:
  - "local baseline 43/50, duet 18/50 … duet's own progression across four host changes was 16 → 25 → 25 → 19 → 30 while local baseline sat at 41–45". — [HANDOFF-GOAL.md:2056-2064](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
  - The 08-27 review summarizes it as "local baseline 40–43 vs duet 18–30". — [REVIEW_2026-08-27.md:34-36](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)
- Effort profile: Pi needed "9 tool calls and 1 write". Duet needed "8 passes, 24 mutations, 42 checks, 1.88M tokens" and shipped an unstartable game. — [notes/host-review-2026-08-24.md:26-29](/Users/maximp/OpenCode/duet_agent/notes/host-review-2026-08-24.md); [HANDOFF-GOAL.md:371-377](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
  - Earlier Phase 0 profile: Pi "one session, 9 tool calls, 1 write" produced a complete 47 KB game scoring 40. Duet took "14 rounds, 64 mutations, 124 checks, 4.3M local tokens" to produce "40% of a game scoring 19". — [HANDOFF-GOAL.md:444-447](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
- Judge's causal account: "The gap is not knowledge … **The gap is whether the code was ever executed before being handed over.**" and "the 29-point spread is almost entirely made of defects that a single playthrough would have surfaced in under a minute." — [findings:456-483](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md)

**Dimensions won and lost**
- 08-25: duet's Correctness score of 9 tied glm and beat Pi's 7, with "the best correctness-to-size ratio of the three" (for example, measured-exact physics). Its losses were Polish 6 (vs 10 and 8) and Features 8 (vs 10 and 9). Named omissions: no title screen or controls hint, a background hue computed but never used, HUD bars overflowing, identical capsule colors. — [HANDOFF-GOAL.md:174-198](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
- 08-26: "duet now wins the tier its machinery addresses (correctness: 8.5–9 vs qwen's 7–8) and loses only the tier nobody in its pipeline demands — its frontier reviewer accepts spec-floor presentation as compliant." The judge's deduction list for duet was "entirely presentation". — [HANDOFF-GOAL.md:127-139](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
- Decomposition of the gap from 38.5 to 47.5: about 4 points of presentation and ambition, with the rest in Features/Code depth, where glm exceeded the spec everywhere (7 levels vs 5, pause, fireworks). — [HANDOFF-GOAL.md:108-112](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
- Earlier rounds show the opposite profile, with Correctness as duet's weakest axis (3, 1 or 2 in 08-21/22 rounds; for example Phase 3: "Correctness went backwards, 5 → 3"). — [HANDOFF-GOAL.md:344-345](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
- In the same-model series, duet became "better than local baseline on everything a reader can check — screens, level density, structure — and worse on everything only a run can check". — [findings:927-933](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md)
- 08-19 defect inventory of the routed build, each passing duet's quality gate:
  - sticky powerup a silent no-op (`CONFIG.paddle.h` → NaN);
  - unreachable music pattern;
  - brick-hit block copy-pasted three times;
  - HUD overlapping bricks;
  - "minimal-compliance level design";
  - self-granted scope cuts;
  - milestone vocabulary leaking into comments;
  - a conservative robustness bar.
  - The judge's "binding lock": duet's gate is plan conformance, whereas frontier quality comes from "product-level judgment". — [arkanoid-frontier-parity-judge-2026-08-19.md:14-40](/Users/maximp/OpenCode/duet_agent/notes/arkanoid-frontier-parity-judge-2026-08-19.md)
- 08-26 decision framework, spelled out before the m4 run:
  - "local-alone 41–43, frontier-alone 45–48, duet 38.5–39 rising".
  - Hypothesis: "the 45+ tier may not be fixable-defect territory but *authorship* — glm wins because the frontier wrote the code; in duet every line is typed by the local model".
  - Branches: ≥45 means the bar is met; 41–44 means the frontier should author critical code; <41 means re-found duet as a single continuous loop. — [HANDOFF-GOAL.md:67-87](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
  - No recorded m4 judging closed this branch.

**Judge drift and noise**
- 08-27 review finding E2: judge noise of about 7 points on a fixed file is roughly 7 times the ±1 bar, and no repeat judgings exist.
  - glm scored 41, 43, 42, 43, 48 and 47.5 on an unchanged file; qwen scored 39–42.5.
  - Detecting a 1-point delta at σ≈2.7 would need about 25 judgings per artifact; at k=3–5 the resolution is ±3. — [REVIEW_2026-08-27.md:232](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)
  - Including the 08-21 blind re-judge value of 40 ([HANDOFF-GOAL.md:1612](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)), glm's recorded range is 40–48.
- 08-25 calibration note: "glm read ~5 hot against its 41–43". — [HANDOFF-GOAL.md:180-181](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
- 08-26 note: "Statistically unchanged from 39 (the instrument itself moved ±1.5 on the baselines)". — [HANDOFF-GOAL.md:125-126](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
- 08-28 addendum correction 3: two judgings 24 h apart on the same rubric "disagreed by 1.5 points on the same artifact and by ~5 on a fixed baseline", so "the current bar (±1) is below the noise floor". — [REVIEW_2026-08-27.md:527-534](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)
  - HANDOFF says the 08-26 duet artifact was a different file (m3b, after repairs), so "same artifact" may refer to the Pi file (41 → 42.5).
- Contradiction: the earlier claim "Rival scores stayed within ±1 across four independent judgings … the rubric held steady" appears in [findings:23-25](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md) and [HANDOFF-GOAL.md:2092](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md). It was contradicted by the later 48/47.5 glm scores and by the 08-27 review.
- Judge-bias channels (finding E7):
  1. the rubric mirrored duet's defect history and penalised a spec-required CUT-NOTES block;
  2. the prompt anchored the judge with "standing frontier-only baseline scores 45–48/50 and local-only 40–42/50";
  3. anonymisation was filename-only, so `<title>`s identified artifacts;
  4. the codex judge defaulted to the lane's own model.
  - Seeds were not recorded. — [REVIEW_2026-08-27.md:237](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)
  - Fixed later: the anchor sentence was removed, `judge --repeat K` added, the median and spread reported, titles blanked. The fix notes that "the instrument's own spread on *unchanged* reference files has reached seven points against a parity bar of one". — [CHANGELOG.md:605-619](/Users/maximp/OpenCode/duet_agent/CHANGELOG.md)
- The standing reference ranges in the dogfood suite: glm "41-48 across five judgings (47.5-48 in the current instrument)" and Pi local "40-42.5 across five judgings". — [references.csv](/Users/maximp/OpenCode/duet_agent/evals/dogfood/results/2026-09-08-roadmap-qualification-dcb11af3cd69/report/references.csv)

**Which "frontier" reference was used**
- August arkanoid references: zcode + glm-5.2 (frontier only), codex + gpt-5.6-luna xhigh, and Pi + omlx-coding (local). — [HANDOFF-GOAL.md:2080-2085](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
- The 09-22 review states: "Every 'frontier' baseline so far is GLM-5.3 Flash … No flagship reference has been run." — [REVIEW_2026-09-22.md:63, 497-500](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-09-22.md)
  - That is accurate for the September dogfood pairs but conflicts with the August glm-5.2 and gpt-5.6-luna arkanoid references.
- `arkanoid_opus`, a hand-built higher ceiling, "carries no score". — [findings:249-251](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md)

### Inferences
- On the only goal-shaped quality instrument, duet never matched the frontier reference. The closest routed results trailed frontier by 9 points (39 vs 48; 38.5 vs 47.5) and trailed local-only Pi by 2–4 points. The bar was missed by far more than the judge noise: even with glm's 40–48 spread, duet's best 39 is below every glm score recorded.
- The quality trajectory shifted from "broken and unfinished" (correctness failures, 11–24/50) to "correct but plain" (38.5–39/50). This fits a design in which the frontier model mostly adjudicates diffs and the local model authors everything, so the frontier's taste never reaches the artifact.
- All quality evidence is from one task type (a single-file browser game). The project itself flagged this as the main way its conclusions could be wrong ([findings:234-240](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md)).

### Gaps
- Not recorded in the files read: the judge's identity or model for the manual August judgings; seeds; any repeat (k>1) judging of the same artifact.
- The exact local model is not recorded beyond "qwen" behind the `omlx-coding` alias. HANDOFF gives a 262k context ([HANDOFF-GOAL.md:2039](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)), while the host review gives a measured 81,920-token window ([host-review:166-167](/Users/maximp/OpenCode/duet_agent/notes/host-review-2026-08-24.md)).
- No duet-vs-duet-frontier-only judging on arkanoid was ever produced ([REVIEW_2026-08-27.md:231](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)).
- Tetris was planned as a second spec, but no judged tetris score was found.
- The arkanoid artifacts themselves (`~/OpenCode/arkanoid_baselines/`) are absent, so the scores cannot be checked against them.

---

## Q2. Cost and token results: local share, frontier spend by stage, the routed-vs-frontier-only ledger, per-run tokens and requests, prefill share, and whether cost was ever priced

### Takeaway
Local share varied widely:
- 95% in the one goal-shaped arkanoid run;
- 16–22% frontier in other August arkanoid runs;
- a 30% mean on HumanEval, with 0 of 52 problems at ≥80% local;
- 25–69% in September dogfood pairs;
- 50.5% in the large supervised queue lineage;
- 6% in the Captain Comic run.

Frontier spend went about 92% to diff review and about 1% to planning. The only routed-vs-frontier-only ledger (08-15) showed the routed lane spending 3.7× more frontier tokens. Total cost was never measured. Local tokens were priced at $0 or left unpriced, and the frontier ran on a flat-rate subscription or placeholder rates; the 08-27 review calls "fraction of the cost" "0/0". September runs report only API-equivalent cloud subtotals, the best being 71.3% below a Pi + glm-5.3-flash run at 2.98× the wall time. Measured on one arkanoid run, 64% of local model wall-clock time was spent before the first output token (prefill).

### Cited Findings

**Frontier spend by stage (live arkanoid goal, 2026-08-21)**
- Frontier tokens by stage: review 260,372 (66%); adversarial_review 102,192 (26%); intervention 27,099 (7%); planning 4,025 (1.0%); audit 0. — [HANDOFF-GOAL.md:1710-1722](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
- Totals: frontier 393,688 vs local 2,042,777, a 16.2% frontier share. The file's verdict: "92% of the premium went to reviewing diffs and 1% to deciding what to build."
- Frontier call-site cadence: review runs per milestone pass and again after every rejection, with up to 4 chunks × 3 provider retries. Adversarial review runs per pass. The audit receives the full diff again. A 4-chunk milestone costs 11 frontier calls. — [REVIEW_2026-08-27.md:171-194](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)

**Local share by run**

| Run (date) | Local share | Source |
|---|---|---|
| HumanEval subset (08-09/10), 52 × 1 | 52/52 passed; mean local 30.4%, median 28.5%, max 79%, 0/52 at ≥80%. The review adds "Stale by 206 commits". Partial file `results-humaneval-32-51.jsonl`: 17/20 passed, mean 46.9%, one problem at 80% | [evals/results-humaneval.jsonl](/Users/maximp/OpenCode/duet_agent/evals/results-humaneval.jsonl) (computed); [REVIEW_2026-08-27.md:89](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md) |
| pytest projects (08-10), 4 × 1 | kvstore fail 40%; lru_ttl pass 70%; tokenizer fail 53%; typed_csv fail 27%. Tokenizer reruns: 20/20 at 81% local, and pass at 21% (separate files) | [evals/results-project.jsonl](/Users/maximp/OpenCode/duet_agent/evals/results-project.jsonl), [results-project-tokenizer-v4.jsonl](/Users/maximp/OpenCode/duet_agent/evals/results-project-tokenizer-v4.jsonl), [results-project-tokenizer.jsonl](/Users/maximp/OpenCode/duet_agent/evals/results-project-tokenizer.jsonl); review: "N=1, cherry-picked re-runs" [REVIEW_2026-08-27.md:90](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md) |
| `duet benchmark` (08-15), 2 cases × 2 lanes | routed 69.5% local (frontier 55,152, local 125,635); frontier-only 14,773 frontier tokens | [evals/results-baseline-2026-08-15.json](/Users/maximp/OpenCode/duet_agent/evals/results-baseline-2026-08-15.json) |
| H.0 long-horizon baseline, goal 310aeb88cb45 + siblings (recorded 08-21) | frontier 5,458,152 / local 19,239,415, i.e. 22.1% frontier; 5 milestones met; 1,091,630 frontier tokens per met step. Caveats: "N=1 and uncontrolled"; cached input excluded. The artifact placed last of three | [evals/results-long-horizon-baseline-2026-08-21.json](/Users/maximp/OpenCode/duet_agent/evals/results-long-horizon-baseline-2026-08-21.json) |
| 08-21/22 arkanoid process metrics | frontier share 16.2% → 9.9% (after verification work) → 21.0% (after throughput work); total tokens for a comparable artifact 4.79M → 4.79M → 1.88M; passes per invocation 1 → 1 → 8 | [findings:27-34](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md) |
| Same-model duet builds (local model in both roles) | §12: "67% local / 33% frontier"; §17: "33% of tokens on the frontier role" | [findings:641-643, 950-951](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md) |
| 08-24 measured run (goal 32f571eec692; not scoreable) | ≈3.1M local + ≈0.4M frontier ≈ 86% local | [HANDOFF-GOAL.md:2423-2425](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md) |
| **08-25 arkanoid m3 (the 39/50 run)** | ≈3.05M local / 165k frontier = **95% local** across five invocations. The review: "the routing clause holds there and nowhere else that was measured" | [HANDOFF-GOAL.md:183-185](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md); [REVIEW_2026-08-27.md:95-96](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md) |
| 09-05 batch 21 (13-test LRU task) | routed local 38.0% of total; routed used 17.2% fewer frontier tokens but 33.5% more total tokens (122,193 vs 91,531) and was 11.7% faster (1,029.6 s vs 1,165.9 s) | [docs/OUTCOME_CALIBRATION_2026-09-04.md:20-30](/Users/maximp/OpenCode/duet_agent/docs/OUTCOME_CALIBRATION_2026-09-04.md) |
| 09-05 batch 22 (KV store) | 142,850 tokens, 40.36% local. A false completion: 20/20 tests passed but 3 supplemental failures | [OUTCOME_CALIBRATION:3-16](/Users/maximp/OpenCode/duet_agent/docs/OUTCOME_CALIBRATION_2026-09-04.md) |
| 09-07 matched pair (LRU/TTL, 16 checks) | hybrid local (15,243 + 1,948) / 69,773 total = 24.6% (computed) | [docs/MATCHED_DOGFOOD_COMPARISON_2026-09-07.md:9-24](/Users/maximp/OpenCode/duet_agent/docs/MATCHED_DOGFOOD_COMPARISON_2026-09-07.md) |
| 09-08 fresh queue qualification (dcb11af3cd69) | local (323,682 + 35,534) / 522,972 = 68.7% (computed); 8 GLM + 12 local requests | [docs/ROADMAP_QUALIFICATION_2026-09-08.md:20-24](/Users/maximp/OpenCode/duet_agent/docs/ROADMAP_QUALIFICATION_2026-09-08.md) |
| 09-13 canaries (13-test task) | 79f9bbb: 6 frontier / 13 local requests, about 61% local (computed); b10b9c4: 8 frontier / 15 local requests, about 61% local | [docs/GOAL_STATUS.md:394-446](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md) |
| 09-16 Captain Comic (goal 89a2deb28023; blocked) | 2 local requests (28,396 tokens) vs 35 frontier requests (442,790 tokens): **94% frontier**. 258,954 tokens went to review/adversarial review | [docs/CAPTAIN_COMIC_HOST_REVIEW_2026-09-16.md:12-18](/Users/maximp/OpenCode/duet_agent/docs/CAPTAIN_COMIC_HOST_REVIEW_2026-09-16.md) |
| 09-18/19 queue-v3 supervised lineage (goal 123656e78e07) | 437 requests, 9,427,637 tokens: 4,666,560 frontier / 4,761,077 local = **50.5% local**; 38 unfinished requests, 12 accounting gaps | [docs/IMPLEMENTATION_STATUS.md:165-172](/Users/maximp/OpenCode/duet_agent/docs/IMPLEMENTATION_STATUS.md); [GOAL_STATUS.md:42-46](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md); [REVIEW_2026-09-22.md:717](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-09-22.md) |
| 09-18 supervised goal 22713160be5b | 202 requests, 4,263,309 tokens; `usage_complete: false` | [docs/DELIVERY_RELIABILITY_REVIEW_2026-09-19.md:57-64](/Users/maximp/OpenCode/duet_agent/docs/DELIVERY_RELIABILITY_REVIEW_2026-09-19.md) |
| 09-22 large goal 68b8f9b03887 checkpoint | 1,800,868 reported tokens, with one unreported local request | [docs/DELIVERY_PLAN.md:352-353](/Users/maximp/OpenCode/duet_agent/docs/DELIVERY_PLAN.md) |

- Review finding E9: "Local share ≥80% is asserted only on arkanoid. HumanEval mean 30% (0/52 ≥80%); projects 27–70%; benchmark 69%. No gate consumes `local_share_pct`." — [REVIEW_2026-08-27.md:239](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)
- Internal inconsistency: [findings:170-171](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md) says "97–99% of tokens run local, frontier share 9.9–21%", and §11 (line 566) repeats "97-99%". Those two figures cannot both describe total tokens.

**Routed vs frontier-only benchmark ledger (08-15)**
- Routed: 1/2 resolved; frontier 55,152 tokens; local 125,635; wall 2,657.9 s.
- Frontier-only: 0/2 resolved; frontier 14,773 tokens; `usage_complete: false`; wall 201.6 s; `estimated_cost: null` in both lanes.
- The routed lane therefore spent 3.73× the frontier tokens and about 13× the wall time (computed). — [evals/results-baseline-2026-08-15.json](/Users/maximp/OpenCode/duet_agent/evals/results-baseline-2026-08-15.json)
- The review's reading: "`no_evidence`; routed spent **3.7× more** frontier tokens than the frontier-only control | Harness ran once". It suspects "the frontier-only rewrite … has likely never worked end-to-end". — [REVIEW_2026-08-27.md:92, 231](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)

**Prefill share of local wall-clock time (measured 08-28)**
- Setup: arkanoid goal 65ce967832fe, run 6, 243 local calls over about 1h50m of a milestone-1 repair drive. — [notes/PREFILL_MEASUREMENT_2026-08-28.md:3-24](/Users/maximp/OpenCode/duet_agent/notes/PREFILL_MEASUREMENT_2026-08-28.md)
  - Total input: 5,011,024 tokens; 65.0% served from cache; 1,752,656 (35.0%) re-prefilled.
  - Local model wall clock: 386.8 min, of which **249.1 min (64%)** passed before the first output token.
  - Uncached prefill rate about 117 tok/s. Median time to first output 42.8 s; worst 443 s.
- By phase: action turns averaged 23,113 input tokens at 69.2% cached, so about 7,000 **new** tokens per turn from the executor "re-reading spans of a 60KB artifact it has already seen". The `report` phase cached only 10.2% (a suspected defect). `mutation_repair` on the narrower route cached 0.0% (a cold KV cache after a model switch), costing 19.4 min over 3 calls. — [PREFILL_MEASUREMENT:26-63](/Users/maximp/OpenCode/duet_agent/notes/PREFILL_MEASUREMENT_2026-08-28.md)
- Caveat: "One goal, one artifact size, one local server." — [PREFILL_MEASUREMENT:75-78](/Users/maximp/OpenCode/duet_agent/notes/PREFILL_MEASUREMENT_2026-08-28.md)
- This measurement contradicts the 08-27 review's root cause #1, which said the prompt was re-prefilled "from byte 0 on nearly every turn" ([REVIEW_2026-08-27.md:39-45](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)). The measurement shows the prefix cache working at about 69%.
- The 09-22 review ties its finding M4 to this: the executor snapshot is "rebuilt at the tail of every request and sized to fill the budget … matches the measured '~7K new tokens/turn'". — [REVIEW_2026-09-22.md:448-459](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-09-22.md)
- September request growth: executor input grew from 24,995 to 56,891 tokens per request (143 s and 563 s per request). An output allowance ratcheted 20,480 → 10,240 → 5,120 → 2,560 → 1,280 → 640 tokens. — [DELIVERY_PLAN.md:85-104](/Users/maximp/OpenCode/duet_agent/docs/DELIVERY_PLAN.md)
- In the 09-08 run, request size grew from 45 KB to 161 KB before compaction cut it to 93 KB; one local response spent 8,193 reasoning tokens on a small change. — [ROADMAP_MATCHED_REFERENCE_2026-09-08.md:77-81](/Users/maximp/OpenCode/duet_agent/docs/ROADMAP_MATCHED_REFERENCE_2026-09-08.md)

**Was cost ever priced?**
- 08-27 review §0: "Local tokens are priced at $0, the frontier route is a flat-rate subscription priced at $0 marginal, every API rate in `evals/dogfood/pricing.json` is an unverified placeholder, and deadline-killed frontier rounds … are recorded as zero tokens. 'Fraction of the cost' is currently 0/0." — [REVIEW_2026-08-27.md:29-32](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)
- Related 08-27 findings:
  - E3: `quality_pct_per_list_dollar` divides by zero. — line 233
  - B3/C4: the usage of cut streams and errors is lost. — lines 161, 193
  - C7: cached input is never priced. — line 196
  - Fix a273abb: unverified zero rates became "unpriced, not free". — [CHANGELOG.md:597-604](/Users/maximp/OpenCode/duet_agent/CHANGELOG.md)
- 08-24 claim, never verified: "the cost axis is effectively solved; throughput is what blocks the quality axis", because a flat-rate frontier route would make the frontier slice nearly free. — [HANDOFF-GOAL.md:2423-2425](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md); [host-review:193-200](/Users/maximp/OpenCode/duet_agent/notes/host-review-2026-08-24.md)
- **09-07 matched pair** (one small task, one run per route):
  - Both lanes passed 16/16.
  - Hybrid took 239.4 s vs 116.6 s (**2.05× slower**).
  - GLM API-equivalent cost: $0.01002685 vs $0.01109785, only **9.65% lower**. The hybrid's cloud subtotal was 90.35% of the control's entire cost.
  - The frontier final audit alone was 42.73% of the hybrid's GLM cost.
  - Verdict: "This task does not support the north-star claim of equivalent results at half the cost with no slowdown."
  - Tariff: $0.15/M input, $0.50/M output. The control is duet running GLM-5.3-Flash for every role, not a standalone frontier agent. — [MATCHED_DOGFOOD_COMPARISON_2026-09-07.md:3-44, 112-114](/Users/maximp/OpenCode/duet_agent/docs/MATCHED_DOGFOOD_COMPARISON_2026-09-07.md)
- **09-08 Duet vs Pi 0.85.1 + zai/glm-5.3-flash** (durable queue, 73 sealed checks + supplemental; both passed):
  - Elapsed: 59m22.1s vs 19m53.8s.
  - Cloud API-equivalent cost: $0.027055 vs $0.094156. "Duet used 71.3% less cloud API-equivalent cost and took 2.98 times as long."
  - Pi's cloud input was 1,855,796 tokens, of which 1,779,328 (96%) were cached.
  - Status: "exploratory matched reference … not the five-pair confirmation cohort and does not establish general frontier parity". Actual billing and Mac electricity are unknown.
  - The electricity break-even for a 50% saving was computed as ≤$0.020023, about 134.91 W average at an illustrative $0.15/kWh ("not a measurement"). — [ROADMAP_MATCHED_REFERENCE_2026-09-08.md:1-59](/Users/maximp/OpenCode/duet_agent/docs/ROADMAP_MATCHED_REFERENCE_2026-09-08.md)
  - Here Pi is a *frontier-only* baseline using a cloud model, unlike August's local-only Pi.
- September canaries report "API-equivalent" frontier cost at frozen rates, for example $0.759124 for 79f9bbb and $0.985100 for b10b9c4. Every one states that "actual subscription billing and local pricing/energy remain unknown". — [GOAL_STATUS.md:394-446](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md)
- 09-22 review verdict item 3: "The cost claim cannot currently be measured by the product itself." Reasons given:
  - M1: one retried 429/5xx makes `charged_tokens()` return `u64::MAX`.
  - M6: cache and setup spend are ignored.
  - M8: four conflicting success definitions exist:
    - `GoalGate`: within 5 points and 4× fewer frontier tokens;
    - `benchmarks/premise`: within 5 points and ≤0.5× list cost;
    - the operator bar: ±1 at ≥80% local;
    - `DELIVERY_PLAN`: "no fixed savings percentage".
  - Pricing rates are still placeholders. — [REVIEW_2026-09-22.md:58-63, 419-502](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-09-22.md)
- The current plan explicitly drops token-share targets: "No fixed savings percentage defines it … Using a cheaper model is an input to the economics calculation, not evidence that Duet achieved the product goal." — [DELIVERY_PLAN.md:3-35](/Users/maximp/OpenCode/duet_agent/docs/DELIVERY_PLAN.md); also [GOAL_STATUS.md:3-5](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md)

### Inferences
- The ≥80% local routing clause was met only once (95% on the 08-25 arkanoid run), and that run missed the quality bar. In every other measured setting local share was below 80%, and on the tasks that passed their checks it usually ranged from 25% to 70%.
- Frontier cost was dominated by fixed coordination: diff review in August, the final audit in September. It does not shrink as more execution moves local. On small tasks this erased most of the savings (−9.65% on 09-07). On a larger task, the savings were real only in cloud API-equivalent dollars (−71.3% on 09-08), at roughly 2–3× the wall time.
- Prefill, not decoding, dominated local latency. Given the repeated context rebuilding the project describes, the local "cheap tokens" cost wall-clock time, and electricity cost was never measured.

### Gaps
- No run in the files has a total cost measured end to end (actual cloud bill plus local energy). All cost figures are cloud API-equivalent estimates or token counts.
- No matched cost comparison exists on the arkanoid quality task.
- No repeated pairs exist (the planned five-pair Pi cohort and eight-pair flagship cohort are "Not qualified"; [GOAL_STATUS.md:769-770](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md)).
- It is unclear whether the 09-18 goal 22713160be5b (4.26M tokens) is a subset of the 9.43M-token lineage for goal 123656e78e07; the files do not say.

---

## Q3. Large-task delivery results in September 2026 (queue-v3 and other large runs; supervised vs autonomous)

### Takeaway
There was one fresh autonomous large pass: 09-08, durable queue, 73/73, no steering. A 09-10 review later showed its 73-check suite missed two concurrency defects, and the run was never re-graded. Every later 73/73 result (09-16, 09-17, 09-18/19) was a supervised checkpoint continuation that needed host corrections, resumes, steering or coordinator interventions, and the project explicitly marks each `fresh_qualification: false`. Fresh queue-v3 attempts stopped in planning or execution (7/73, 39/73, 40/73). The current large goal is stopped at 40/73 with 2 of 7 work orders done. Small canaries (13 or 16 checks) completed autonomously several times, but often stalled or blocked despite passing checks.

### Cited Findings
- **09-08 fresh autonomous qualification**: candidate dcb11af3cd69, goal 5767de0a6b82, completed "in one invocation without steering, workspace edits, configuration changes or restarts".
  - 73/73 sealed checks (61 visible + 12 held out) plus the supplemental check passed.
  - Elapsed 59m22.1s; 20 requests.
  - The local executor passed 59/61 native tests on its first check, then repaired autonomously. — [docs/ROADMAP_QUALIFICATION_2026-09-08.md:1-36](/Users/maximp/OpenCode/duet_agent/docs/ROADMAP_QUALIFICATION_2026-09-08.md)
- **09-10 coverage review**: "73/73 did not establish a correct artifact or autonomous delivery … its independent artifact suite missed two concrete concurrency defects" (a non-atomic export snapshot and cancellation overwriting completion). The candidate examined was goal 4cf5dceeaddf. — [docs/ACCEPTANCE_COVERAGE_REVIEW_2026-09-10.md:3-19](/Users/maximp/OpenCode/duet_agent/docs/ACCEPTANCE_COVERAGE_REVIEW_2026-09-10.md)
  - The 09-22 review: the 09-08 fresh pass "was never re-graded after the 73-check suite was shown (09-10) to miss concurrency defects". — [REVIEW_2026-09-22.md:700-702](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-09-22.md)
- **09-12**: three supervised planning attempts stopped before implementation (invalid JSON at 850.651 s; a 900 s deadline; a controlled 1,200 s deadline). Each retained 7/73 sealed and 0/4 supplemental checks. — [GOAL_STATUS.md:453-464](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md)
- **09-13 queue-v3, run 092917** (goal fbd75955a8f7):
  - Ended **blocked** at 40/73 sealed, 0/4 supplemental, after 63.1 min.
  - A frontier intervention never dispatched: its wire request needed 85,874 bytes against an 84,000-byte limit.
  - Usage: 1 frontier request, 15 local requests. — [GOAL_STATUS.md:721-744](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md)
- **09-13 queue-v3, run 114008**: stopped after 72.3 min of "repeated unchanged inspection" at 39/73, 0/4. — [GOAL_STATUS.md:356-365](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md)
- **09-13 small canaries** (13 independent checks):
  - Qualified completions: 79f9bbb (29.8 min, 4 mutations) and b10b9c4 (52.7 min, 5 mutations, one autonomous review repair). — [GOAL_STATUS.md:394-446](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md)
  - Unqualified despite 13/13 checks passing: 7ccb1153 (4,129.5 s, stopped); c55d623 (3,783.8 s, blocked on malformed reviewer JSON); cf5b239 (6,246.8 s, stall); ad85ff8 (8,161.7 s, stopped); 04a44b5 (5,724.7 s, blocked). — [GOAL_STATUS.md:247-354, 616-719](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md)
  - 794cb49 stopped at 7/13. — [GOAL_STATUS.md:277-293](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md)
- **09-16 large queue**, goal 49cc83438946: completed natively; 73/73 plus 4/4 supplemental; last continuation 251.2 s.
  - "This is **supervised checkpoint continuation**, with `fresh_qualification: false`."
  - Supervision changed duet, resumed checkpoints, queued completion direction through steering, and restored three saved runtime fields. — [docs/LARGE_DOGFOOD_RESULT_2026-09-16.md:3-35](/Users/maximp/OpenCode/duet_agent/docs/LARGE_DOGFOOD_RESULT_2026-09-16.md)
- **09-16 Captain Comic** (goal 89a2deb28023): blocked after 74m37s with one guarded file creation; 94% frontier tokens. The candidate "became a comic reader instead of a platform game". — [CAPTAIN_COMIC_HOST_REVIEW_2026-09-16.md:5-18, 40-41](/Users/maximp/OpenCode/duet_agent/docs/CAPTAIN_COMIC_HOST_REVIEW_2026-09-16.md)
  - "The Captain Comic run remains unqualified." — [GOAL_STATUS.md:90-99](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md)
- **09-17 durable queue resume** (f771c26287ce): completed after 18 iterations and **17 coordinator interventions**. The fresh sealed rerun passed 61/61 + 12/12, whereas the original sealed grade was 7/61, 0/12. "Not a fresh autonomous Duet qualification." — [GOAL_STATUS.md:62-80](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md)
- **09-17 fresh queue-v3 attempts** all stopped with no delivery:
  - fcfe83d: planning, 675.7 s, over 4.6 MB of reasoning without submitting a plan;
  - 6096ab1: 112.2 s and 218.8 s;
  - b045e98: 397.9 s, over 320 KB of reasoning;
  - 1cb05b9: executor loop on scratch probes. — [GOAL_STATUS.md:137-176](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md)
- **09-17 micro experiments**: `micro_edit`, `micro_dependency` and `micro_repair` "each completed autonomously with passing independent checks". A no-change run completed in 1,526.8 s with 5/5 checks. That "does not qualify the large suite or any cost-saving claim". — [GOAL_STATUS.md:124-135](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md)
- **09-18/19 queue-v3**:
  - Goal 22713160be5b reached 73/73 as supervised development, even though its directory is named "fresh-qualification". Work orders failed repeatedly on executor-generated checks. — [DELIVERY_RELIABILITY_REVIEW_2026-09-19.md:27-64](/Users/maximp/OpenCode/duet_agent/docs/DELIVERY_RELIABILITY_REVIEW_2026-09-19.md)
  - Goal 123656e78e07 completed 61/61 + 12/12 with `fresh_qualification: false`; 437 requests, 9.43M tokens. — [GOAL_STATUS.md:31-46](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md)
  - The 09-22 review: "The 09-18 'fresh-qualification' directory became supervised development … and is never counted as a failed fresh attempt. That breaks the roadmap's own 'count every failed cohort attempt' rule." — [REVIEW_2026-09-22.md:703-706](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-09-22.md)
- **09-21/22 current large goal** 68b8f9b03887:
  - "incomplete, with 2/7 work orders passed and **40/73 independent checks**" (36/61 visible, 4/12 held out); the CLI supplemental check fails; "the large run is stopped".
  - "Fresh autonomous delivery and comparative total cost remain unqualified." — [GOAL_STATUS.md:7-15](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md); [DELIVERY_PLAN.md:63-74, 341-346](/Users/maximp/OpenCode/duet_agent/docs/DELIVERY_PLAN.md)
- 09-22 review results timeline, in its own words:
  - 09-05: 13/13; a false KV completion.
  - 09-07: 16/16.
  - 09-08: fresh 73/73 and Duet vs Pi 73/73.
  - 09-09 to 09-14: "many blocked, failed or partial runs".
  - 09-16: supervised 73/73 + 4/4.
  - 09-17: 61/61 + 12/12 with 17 interventions.
  - 09-18/19: supervised 73/73, 9.4M tokens.
  - 09-21/22: 40/73, stopped. — [REVIEW_2026-09-22.md:707-718](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-09-22.md)
  - The review also flags the README as out of date: it still cites 73/73 as the latest large dogfood. — [REVIEW_2026-09-22.md:692-698](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-09-22.md); [README.md:29-33](/Users/maximp/OpenCode/duet_agent/README.md)
- Qualification status table as of 09-22 ([GOAL_STATUS.md:759-771](/Users/maximp/OpenCode/duet_agent/docs/GOAL_STATUS.md)):
  - Fresh delivery: "Not qualified".
  - Pi repeatability/economics: "Not qualified", needs "five frozen accepted pairs".
  - Flagship quality/measured cost: "Not qualified", needs "eight matched pairs and measured energy".
- Earlier 22-batch diagnostic series (09-04/05): "independently accepted counts remain one routed and two control". — [OUTCOME_CALIBRATION_2026-09-04.md:11-14](/Users/maximp/OpenCode/duet_agent/docs/OUTCOME_CALIBRATION_2026-09-04.md)

### Inferences
- In September the project shifted from the quality goal (arkanoid judgings) to a reliability goal: delivering a large multi-file task end to end. Even there, autonomous completion was a single event (09-08). The large task then failed or stalled repeatedly as the harness changed, and its later successes needed human supervision. The recurring failure modes were planner reasoning runaways, request-size and wire limits, stalls where the executor repeated unchanged reads, and schema or protocol mismatches between roles.
- Coordination between the frontier and local roles was itself a major source of failure. Cases include 17 coordinator interventions, the unsized intervention request, malformed reviewer JSON that blocked a goal whose checks had passed, and orchestrator-executor handback schema conflicts.

### Gaps
- No September large-task run was scored for product quality beyond pass/fail checks, and no blind judgment was made.
- No fresh autonomous queue-v3 attempt succeeded after 09-08.
- Total failed-attempt cost is not aggregated anywhere.

---

## Q4. The project's own root-cause diagnoses

### Takeaway
The project's reviews agree. The shortfall came from duet's host architecture, not model capability. The phase machine and pass boundaries fragment one task into memoryless passes. Frontier tokens go to reading diffs instead of authoring content or observing the running program. Refusals and process limits impede the executor. Quality features ship off by default. The host review found that every point gained came from *removing* host friction. The 09-22 review adds that completion integrity, the sandbox boundary and cost accounting all have concrete gaps, the gates are red and no CI runs, and that the cost claim "cannot currently be measured".

### Cited Findings

**2026-08-27 review, five root causes** ([REVIEW_2026-08-27.md:34-68](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md))
1. The local prompt is re-prefilled from byte 0 on nearly every turn, because the phase name sits in the system string and the tools JSON is rewritten each turn.
   - The 08-28 prefill measurement contradicts this: the cache hit rate was about 69%, and the true cost is about 7K new tokens per turn from re-reads ([PREFILL_MEASUREMENT:37-50](/Users/maximp/OpenCode/duet_agent/notes/PREFILL_MEASUREMENT_2026-08-28.md)).
2. The frontier reads the same diff 3–11 times per milestone. "92% of frontier spend is diff review; 1% is deciding what to build."
3. When the falsification round blows its ~350 s deadline, the accepted criteria verdict is thrown away. Every criterion is marked Unmet, and the request is retried twice with the identical deadline, unbilled.
   - Observed live: the z.ai adversarial round exceeded its deadline nine times on 08-26 and five times on 08-25. The goal "never credited its milestones". — [HANDOFF-GOAL.md:153-158, 194-198](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
4. The unit of work is a pass that ends on any of 22 host limits, with one local repair per pass. Each pass end costs a frontier review plus a full context rebuild. This is "the documented cause of the triplicated game loop and the 8–14-pass, 1.9–4.8M-token runs."
5. Frontier judgment is applied to a checklist over a diff, never to the spec or the running program.
   - Every quality-lifting mechanism defaults off: `executor_investigation`, `executor_instruments`, `executor_proposed_checks`, `review_sees_request`, `review_sees_artifact_run`, `orchestrator_steering`, `behavior_needs_observation`, `local_first_review`. "A fresh install ships the ~20/50 configuration."
   - Recommendation: "stop spending on the phase machine and the diff-review stack … re-found the executor as a single stable-prefix conversation with five tools, move frontier spend … to authoring the design brief and judging host-observed evidence." — lines 70-75

**Default-off quality features**
- The 08-27 list: `review_sees_request`, `review_sees_screenshot`, `review_sees_artifact_run`, `orchestrator_steering`, `executor_investigation`, `executor_instruments`, `executor_proposed_checks`, `local_first_review`, `behavior_needs_observation`, `product_acceptance`, `require_observed_work`, `scratch_surface` and `cost_denominated_budgets` are all false. The reason given is replay-fixture byte identity: "the measured configuration and the shipped configuration are different programs." — [REVIEW_2026-08-27.md:98-108](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)
- 08-28 addendum: "all `= false` … A fresh install is still the ~20/50 configuration … nothing has moved it." — [REVIEW_2026-08-27.md:480-485](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)
- 09-04 review F09: completion quality depends on opt-in policies that default false. F08: the central value proposition lacks a current controlled result. — [docs/END_TO_END_REVIEW_2026-09-04.md:141-143](/Users/maximp/OpenCode/duet_agent/docs/END_TO_END_REVIEW_2026-09-04.md)
- 09-22 review C2: `behavior_needs_observation` still defaults to false, so "Behavioural criteria can complete on the reviewer's word alone". F09 is still open. — [REVIEW_2026-09-22.md:174-187, 906-911](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-09-22.md)

**Host review (2026-08-24): where score gains came from** ([notes/host-review-2026-08-24.md](/Users/maximp/OpenCode/duet_agent/notes/host-review-2026-08-24.md))
- "Every point of duet's 11-point deficit is **host-imposed**." (lines 25-27)
- Diagnosis: duet "inserts host judgment between the model and reality at 216 refusal sites"; everywhere that judgment is wrong, duet does worse than having no host. (lines 31-36)
- "Score trajectory … 16 → 25 (remove hard turn caps) → 30 (let the executor look at its own work + record what it ran). **Every point duet has ever gained came from removing host friction, not adding host intelligence.** The six host-authored behavioural checks … all failed validation and were reverted." (lines 48-53)
- Keep: the mutation contract ("never violated in any measured run"), journals and replay, the redaction boundary, and the discrimination bar. (lines 60-81)
- Ranked fixes with predicted points (predictions, not measurements):
  1. the host drives the artifact, +4–6;
  2. refusals teach, +2–4;
  3. frontier spend moves to plan content and observation, +2–3 (92% review / about 1% planning);
  4. executor session continuity, +1–2 (lines 87-172).
- Its stated falsifier: "If items 1–3 land and the score does not reach at least local baseline's 40, the next review should question the phase machine itself." (lines 204-210)
  - Items 1–3 landed on 08-24/25 ([HANDOFF-GOAL.md:214-248](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)). The next scores were 39 and 38.5, just under 40.

**Pass-boundary and context-rebuild effects**
- Finding A4: pass handoff persists host receipts only, never history, tool results or reasoning. The next pass rebuilds a 10–30K-token item 0. — [REVIEW_2026-08-27.md:145](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)
- The triplicated brick-hit loop is "the signature of transactional, memoryless passes". — [HANDOFF-GOAL.md:1758-1761](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
- "Each fix succeeded and the total moved +4 … no participant ever holds the whole task." — [findings:156-168](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md)
- Chaining repair rounds cut passes from 1 to 8 per invocation and tokens from 4.79M to 1.88M. — [findings:85-89](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md)
- "Every pass boundary re-buys context and review rounds." — [host-review:157-162](/Users/maximp/OpenCode/duet_agent/notes/host-review-2026-08-24.md)
- Knowledge continuity: "Today duet has a horizon of one pass, repeated with excellent bookkeeping"; "roughly 94 manual resumes" for one goal. — [HANDOFF-GOAL.md:1770-1776](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md); [notes/long-horizon-plan-2026-08-20.md:205-207](/Users/maximp/OpenCode/duet_agent/notes/long-horizon-plan-2026-08-20.md)
- Pi's harness, as read from its source: four tools, a `while(true)` loop, no verification instruction, and an 8:1 verification-to-write ratio "from the model's own judgment". Duet: "8 phases, per-phase surfaces … Every boundary is a lossy serialization". — [HANDOFF-GOAL.md:485-509](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
- September equivalent: the executor snapshot is rebuilt every request (09-22 M4). The context materializer selected stale predecessor failures as the current next action. — [DELIVERY_PLAN.md:321-328](/Users/maximp/OpenCode/duet_agent/docs/DELIVERY_PLAN.md)

**Other self-diagnoses**
- The 08-22 prediction, recorded before the result: "if defragmentation does not lift the score past 40, the split model is not recoverable, and duet should be re-founded as a single-agent loop". The Phase 3 score of 24 met that condition. — [HANDOFF-GOAL.md:364-399](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
  - A re-founding along those lines was recommended again by the 08-27 review. The later September architecture (work orders, coordinator, virtual context) kept the split ([DELIVERY_PLAN.md:13-35](/Users/maximp/OpenCode/duet_agent/docs/DELIVERY_PLAN.md)).
- "Planning plus audit cannot buy frontier quality from 90% local work": a frontier model at the endpoints is "blind between checkpoints". — [findings:529-538](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md)
- Authorship hypothesis: the 45+ tier may require frontier-authored code. — [HANDOFF-GOAL.md:69-73](/Users/maximp/OpenCode/duet_agent/HANDOFF-GOAL.md)
- "More sophisticated verification in a simulated environment is worse than less verification in the real one." — [findings:677-678](/Users/maximp/OpenCode/duet_agent/notes/findings-2026-08-22.md)
- The one-shot failure is infrastructure: "every stop but one was infrastructure, not work … Each was 'fixed' by a human typing `resume`". — [REVIEW_2026-08-27.md:551-559](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)
- 09-19 reliability review: "Several recent safeguards constrain the model's next decision without preserving enough information to execute that decision exactly"; patches improve safety "but sometimes create new obstacles to" progress. — [DELIVERY_RELIABILITY_REVIEW_2026-09-19.md:11-25](/Users/maximp/OpenCode/duet_agent/docs/DELIVERY_RELIABILITY_REVIEW_2026-09-19.md)
- 09-07 cost diagnosis: "Frontier coordination dominates the remaining cost … Local execution cannot remove that fixed coordination expense by itself." — [MATCHED_DOGFOOD_COMPARISON_2026-09-07.md:36](/Users/maximp/OpenCode/duet_agent/docs/MATCHED_DOGFOOD_COMPARISON_2026-09-07.md)

**2026-09-22 review verdict (§1, §10)** ([notes/REVIEW_2026-09-22.md:32-89, 841-898](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-09-22.md))
- The guard primitives are strong.
- "Both headline claims have concrete gaps":
  - Completion: three completion paths exist, and one (the executor-budget audit) skips review and every host veto (C1). Behavioural criteria complete on the reviewer's word. Evidence is matched by kind, not relevance. Above 4,096 source files no host check is admissible.
  - Boundary: `.git` is writable, and a trusted project config can reroute API keys.
- "The cost claim cannot currently be measured by the product itself."
- "The gates are red and nothing is watching them": 1 clippy error; 11 deterministic test failures plus a stack overflow skipping 138 tests; CI deleted 2026-08-30.
- "The docs have slipped back into a session log": 100 files, and six documents claim authority.
- Recommended order: green gates and CI → one completion owner → close the OS boundary → a `.duet` path registry → make cost measurable (including "Run one flagship frontier reference") → bind evidence to criteria → fix the context tail ("where the measured 64% prefill share comes from") → docs.
- Scope caveat: "No live goal" was run for the review, and the 8.7K uncommitted lines were not reviewed. — lines 6-28
- Follow-up log: on branch `fix/green-gates`, 3,269 tests pass and 3 fail; not merged. — lines 915-938

### Inferences
- The project's own record supports this causal chain: fragmented passes and host friction → lost context and re-reading → local prefill-heavy cost and weaker artifacts; frontier budget spent on diff adjudication → no frontier authorship or observation → "correct but plain" output. The sources conflict on whether the fix is to re-found as a single loop (08-22 prediction, 08-27 recommendation) or to consolidate the existing split engine (09-19, 09-22). The later documents chose consolidation.
- Several diagnoses reverse earlier ones. The prefill cause moved from cache-busting to re-reads, and the judge's stability from "±1" to "~7 points". Individual flags such as `require_observed_work` looked like failures solo but helped in combination. So single diagnoses were N=1 and often revised.

### Gaps
- No measured experiment isolates the "authorship" hypothesis, in which the frontier writes critical code; it was planned but never run.
- The 08-27 Tier 0/1 plan (a stable-prefix single conversation) has no recorded outcome measurement on the arkanoid quality task.

---

## Q5. Scale of the project (lines of code, commits, docs, time span)

### Takeaway
Duet is a single-author Rust codebase of about 300K committed lines in `src/`, built in about six weeks (first commit 2026-08-12, HEAD 2026-09-22). It has 453 commits, peaking at 45 in one day, and about 100 docs files. That is large relative to the size of the measured results: N=1 judgings and a handful of qualification runs.

### Cited Findings
- 08-27 review §1.4:
  - 224,298 lines of Rust in `src/` (17,076 of them in `tests.rs` files) + 29,087 in `tests/`; 2,475 `#[test]`s.
  - `src/provider/mod.rs` 17,192 lines; `src/main.rs` 13,133; `src/executor/tests.rs` 12,125.
  - "Single author, 206 commits since 08-12 (peak 28/day)." — [REVIEW_2026-08-27.md:122-129](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)
- 08-24 host review: "94 modules / 222,858 lines". — [host-review:13](/Users/maximp/OpenCode/duet_agent/notes/host-review-2026-08-24.md)
- 09-22 review:
  - 300K lines of Rust in `src/` (187 files), 32K lines of integration tests, 100 files in `docs/` (about 1.5 MB). — [REVIEW_2026-09-22.md:8-11](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-09-22.md)
  - Growth: `src/` went from 200K lines (08-13) to 229K (08-29) to 300K (09-22). Since 09-01 it gained +80.6K/−18.0K lines, while `tests/` gained only +4.8K and `docs/` +23K. 3,303 test functions.
  - Largest units: `run_active_goal_inner` 5,219 lines; `provider/mod.rs` 21.1K; `main.rs` 13.9K.
  - 17 `include!` files (49.5K lines) are hidden from rustfmt. — [REVIEW_2026-09-22.md:806-818](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-09-22.md)
  - Docs count: 3 files on 08-13, 9 after the 08-28 cleanup, 38 on 09-08, 83 on 09-15, 100 on 09-22. 88 ship in the crate, including 53 dated session reports, `IMPLEMENTATION_TRACKER.md` (332 KB) and `DELIVERY_PLAN.md` (194 KB). — [REVIEW_2026-09-22.md:648-659](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-09-22.md)
- Measured for these notes, read-only, on 2026-09-22 at HEAD `5f83b89`:
  - `git rev-list --count HEAD`: **453 commits**.
    - First commit: 2026-08-12 `c1d1bc1` "Initial open-source release".
    - Last commit: 2026-09-22 `5f83b89` "Integrate mandatory Git history and delivery safeguards".
    - 451 commits by maximp@gmail.com, plus 2 under noreply addresses of the same name.
    - 230 commits since 2026-08-28.
    - Busiest days: 45 commits on 2026-09-17, 37 on 2026-09-18, 28 on 2026-08-16.
  - Committed `src/**/*.rs` at HEAD: **300,352 lines** in 186 files. The working tree has 310,119 lines, including 9,778 uncommitted insertions in 19 files.
  - `tests/**/*.rs`: 32,047 lines.
  - Test attributes: 2,555 `#[test]` and 1,019 `#[tokio::test` (grep counts, not deduplicated).
  - `docs/` at HEAD: 100 files (88 top-level `.md`). `notes/`: 17 files.
  - `HANDOFF-GOAL.md`: 2,590 lines. `docs/DELIVERY_PLAN.md`: 2,925 lines.
  - Measured with `git rev-list`, `git log`, `git shortlog`, `git ls-tree`/`git cat-file | wc -l`, and `git diff --stat`.
- Time span of recorded evidence: HumanEval results dated 08-09/10 predate the repo's first commit (08-12 "Initial open-source release"). The last evidence is 09-22. — [evals/results-humaneval-0-11.jsonl](/Users/maximp/OpenCode/duet_agent/evals/results-humaneval-0-11.jsonl) (file mtime Aug 9); [REVIEW_2026-08-27.md:89](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-08-27.md)
- The 09-22 review notes that the README's install command cannot work publicly (`duet-core` is not on crates.io; the GitHub URL returns 404 unauthenticated). — [REVIEW_2026-09-22.md:79-80](/Users/maximp/OpenCode/duet_agent/notes/REVIEW_2026-09-22.md)

### Inferences
- About 300K lines and about 450 commits in six weeks, from one author, against a result base of N=1 blind judgings and a handful of fresh qualification runs. The effort went into mechanism much faster than into outcome measurement. The 09-04 review names this directly: "mechanism development has outrun outcome measurement" ([END_TO_END_REVIEW_2026-09-04.md:141](/Users/maximp/OpenCode/duet_agent/docs/END_TO_END_REVIEW_2026-09-04.md)).

### Gaps
- Development history before 2026-08-12 is not in this repository's git log, so the true project start date and effort are unknown.
- Line counts include large test modules inside `src/`: 17K lines of `tests.rs` on 08-27, and `main.rs` is "94% tests" on 09-22. The production-only size is therefore smaller than the headline figures.
