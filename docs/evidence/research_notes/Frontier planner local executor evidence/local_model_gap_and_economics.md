# Local open-weight models vs frontier on agentic coding, and the real time/money cost of local execution (Apple Silicon)

All figures were observed on 2026-09-22 unless noted otherwise. "Measured" means a number published by the source. "Inference" means my own arithmetic or interpretation, and those appear only under the Inferences headings. Figures dated before 2026 are marked **[older]**. Vendor self-reported scores are marked **[vendor]**. Aggregator-only claims are marked **[aggregator, unverified]**.

## Q1. Capability gap (2025–2026): how far do locally runnable open-weight models trail frontier models on SWE-bench Verified, Terminal-Bench, Aider Polyglot, LiveCodeBench and METR time horizons, and is the gap bigger on long-horizon multi-turn work than on single-shot code generation?

### Takeaway
When the harness is the same for every model, local-class models (≤~120B total, 3–5B active or ≤32B dense) trail the frontier by about 20–50 points on SWE-bench Verified. Vendors report much smaller gaps using their own harnesses. The gap widens as tasks get longer and more multi-turn: it is small on LiveCodeBench, which is single-shot, and roughly 3× or more on METR time horizon. Independent long-horizon measurements of 2026 local models do not exist.

### Cited Findings
**Same-harness SWE-bench Verified (mini-SWE-agent / "bash-only", run by the SWE-bench team).** Leaderboard data file last committed 2026-09-01; the newest entries are from Feb 2026.
- Frontier: Claude 4.5 Opus (high) 76.8%, $0.754/instance, 32.9 model calls/instance (2026-02-17). Claude 4.6 Opus 75.6%, $0.552, 28.9 calls. Gemini 3 Flash 75.8%. GPT 5.2 (high) 72.8%. GPT 5.2 Codex 72.8%, 28.1 calls. Claude 4.5 Sonnet (high) 71.4%. GPT 5 (medium) 65.0% (2025-08-07) — [SWE-bench leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json), [swebench.com](https://www.swebench.com)
- Large open-weight models, not local-class: MiniMax M2.5 75.8%, $0.073/instance, 60.5 calls. GLM 5 72.8%, 76.2 calls. Kimi K2.5 70.8%. DeepSeek V3.2 70.0%, 88.5 calls (all 2026-02-17). Qwen3-Coder 480B/A35B 55.4% (2025-08-02) **[older]** — [SWE-bench leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)
- Local-class: Devstral Small (2512, 24B dense) 56.4% with 86.9 calls/instance (2025-12-09). gpt-oss-120b 26.0% (2025-08-07, mini-SWE-agent v1.7.0) **[older]**. Qwen2.5-Coder 32B 9.0% (2025-08-03) **[older]**. The same file lists other-harness submissions: OpenHands + Qwen3-Coder-30B-A3B-Instruct 51.6% (2025-08-05) and EntroPO + R2E + Qwen3-Coder-30B-A3B 60.4% (2025-09-01, a fine-tuned variant) **[older]** — [SWE-bench leaderboards.json](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)

**Vendor-reported scores are well above the same-harness ones:**
- gpt-oss-120b: 62.4% SWE-bench Verified in OpenAI's model card (Aug 2025) **[vendor]** — [gpt-oss model card](https://arxiv.org/abs/2508.10925). Same-harness result: 26.0% — [SWE-bench data](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json). A user running it via Ollama in mini-SWE-agent got 1 of 10 instances. The maintainer said 10 instances is too small a sample and that results differ between mini-SWE-agent v1.7 and v2 (2026-03-31) — [mini-swe-agent #798](https://github.com/SWE-agent/mini-swe-agent/issues/798)
- Devstral Small 2 (24B): 68.0% **[vendor]**, released 2025-12-09 — [Mistral](https://mistral.ai/news/devstral-2-vibe-cli/), [HF card](https://huggingface.co/mistralai/Devstral-Small-2-24B-Instruct-2512). Same-harness: 56.4% — [SWE-bench data](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)
- Qwen3-Coder-480B: 69.6% with OpenHands vs 55.4% with mini-SWE-agent (both in the SWE-bench file, Aug 2025) **[older]** — [SWE-bench data](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)

**2026 local-class models (vendor-reported only; no independent same-harness runs found):**
- Qwen3-Coder-Next (80B total / 3B active, hybrid attention). The technical report (2026-03-03) runs every model in the same table on the same scaffolds **[vendor-run]**:
  - SWE-bench Verified: 70.6 (SWE-Agent) / 71.1 (MiniSWE-Agent) / 71.3 (OpenHands), vs Claude Opus 4.5 at 78.2 / 77.8 / 79.0.
  - SWE-bench Pro: 42.7 / 38.7, vs Opus 4.5 at 51.6 / 50.2.
  - Terminal-Bench 2.0: 34.2 (Terminus2-xml), 36.2 (Terminus2-json), 30.9 (Claude Code), 25.8 (Qwen Code), vs Opus 4.5 at 58.4 / 57.3 / 53.9 / 51.7.
  - Source: [Qwen3-Coder-Next report](https://arxiv.org/html/2603.00729v1)
- The same report shows Kimi-K2.5 scoring 49.4 on TB2.0 under Terminus2-json and **0.9 under the Claude Code harness**. Harness mismatch can wipe out an open model's score — [Qwen3-Coder-Next report](https://arxiv.org/html/2603.00729v1)
- Qwen3.6-35B-A3B (35B total / ~3B active, April 2026) **[vendor]**. Internal scaffold, temp 1.0, 200K context for SWE-bench; Terminus-2, 256K context, 5 runs averaged for TB2.0 — [Qwen3.6-35B-A3B card](https://huggingface.co/Qwen/Qwen3.6-35B-A3B)

  | Model | SWE-bench Verified | SWE-bench Pro | Terminal-Bench 2.0 | LiveCodeBench v6 |
  |---|---|---|---|---|
  | Qwen3.6-35B-A3B | 73.4 | 49.5 | 51.5 | 80.4 |
  | Qwen3.5-27B (dense) | 75.0 | 51.2 | 41.6 | 80.7 |
  | Qwen3.5-35B-A3B | 70.0 | — | 40.5 | 74.6 |
  | Gemma4-31B | 52.0 | 35.7 | 42.9 | 80.0 |
  | Gemma4-26B-A4B | 17.4 | 13.8 | 34.2 | 77.1 |

**Terminal-Bench:**
- The official tbench.ai leaderboard now serves **Terminal-Bench 4.0** (n_trials = 330 on the entries I checked). Top entries:
  - GPT-6 Astra (Codex, max) 58.18% (2026-09-03)
  - Fable 5.1 (Claude Code, max) 57.88% (2026-09-01)
  - Opus 5 (Claude Code, xhigh) 53.94% (2026-07-24)
  - GLM-5.3 (Claude Code, max) 41.82% (2026-08-14) — the only Z.ai/open-family entry
  - No model that fits in 32–128 GB is listed.
  - Source: [tbench.ai leaderboard](https://www.tbench.ai/leaderboard/terminal-bench/2.0) (embedded data parsed 2026-09-22)
- Token use per TB 4.0 trial, from the same data:
  - Opus 5 xhigh: 6,897,141,993 total tokens and $6,086.22 over 330 trials.
  - GLM-5.3 max: 8,677,039,373 tokens and $2,727.63.
  - GPT-6 Astra max: 1,529,778,322 tokens and $3,267.18.
  - Source: [tbench.ai](https://www.tbench.ai/leaderboard/terminal-bench/2.0)
- TB 2.0 aggregator figures: GPT-5.5 at 0.827 is top and GLM-5.1 at 0.690 is the top open model. Kimi K2 Thinking scores 36%, Qwen3-Coder-480B 23.9 and Kimi-K2-Instruct 27.8 **[aggregator, unverified]** — [llm-stats TB2](https://llm-stats.com/benchmarks/terminal-bench-2), [Tmax paper](https://arxiv.org/pdf/2606.23321). I could not retrieve the official TB 2.0 board.

**Aider Polyglot** (leaderboard last updated 2025-11-20) **[older]**:
- GPT-5 (high) 88.0%, edit format 91.6%
- Gemini 2.5 Pro (32k think) 83.1%
- DeepSeek V3.2 Reasoner 74.2%
- Claude Opus 4 (32k think) 72.0%
- Qwen3 235B 59.6%
- Kimi K2 59.1%
- gpt-oss-120b (high) 41.8%, correct edit format only 79.1%
- Qwen3 32B 40.0%, correct edit format 83.6%
- Source: [Aider leaderboard](https://aider.chat/docs/leaderboards/)

**SWE-Bench Pro** (Scale AI, arXiv v2 2025-11-14) **[older]**. SWE-Agent, same prompt for all models; open-weight models were served on vLLM using "syntax parsing to enable tool-use":
- Public set, 50 turns: Claude Sonnet 4.5 43.6%, Claude Sonnet 4 42.7%, GPT-5 (high) 41.8%, Claude Haiku 4.5 39.5%, Kimi K2 Instruct 27.7%, GPT-OSS 120B 16.2%.
- With a 50-turn and $2 cap (Table 5): GPT-5 (medium) 23.3, Opus 4.1 22.7, Sonnet 4 17.6, GPT-OSS 20B 16.2, Gemini 2.5 Pro 13.5, **Qwen-3 32B 3.4**.
- Source: [SWE-Bench Pro](https://arxiv.org/html/2509.16941v2)

**METR time horizon (50% success)**, raw YAML files downloaded 2026-09-22; page last updated 2026-05-08:
- v1.0 suite **[older]**:
  - gpt-oss-120b 45.1 min [CI 19.9–85.5], released 2025-08-05
  - GPT-5 130.8 min [67.7–247.8], 2025-08-07
  - Claude Opus 4.1 109.3 min
  - Kimi K2 Thinking 57.6 min [27.4–100.6], 2025-11-06
  - Claude Opus 4.5 246.8 min, 2025-11-24
  - GPT-5.1-Codex-Max 162.3 min
  - DeepSeek R1-0528 32.8 min
  - Qwen2.5-72B 5.8 min
  - Source: [METR v1.0 data](https://metr.org/assets/benchmark_results_1_0.yaml)
- v1.1 suite (current), no open-weight models measured:
  - Claude Opus 4.6 718.8 min [316.7–3633.8] (2026-02-05)
  - GPT-5.4 341.7 min (2026-03-05)
  - Gemini 3.1 Pro 384.1 min
  - Claude Mythos Preview 1044.8 min (2026-04-07)
  - Doubling time since 2023: 128.7 days [104.4–158.0]
  - METR warns that "measurements above 16 hrs are unreliable".
  - Source: [METR v1.1 data](https://metr.org/assets/benchmark_results_1_1.yaml), [METR page](https://metr.org/time-horizons/)

**Aggregate lag estimates:**
- Open-weight models have trailed frontier closed models by an average of 4 months (8 ECI points) since Jan 2026. Published 2026-05-29 — [Epoch AI](https://epoch.ai/data-insights/open-closed-eci-gap)
- Models that fit a single consumer GPU (≤28B for RTX 4090, ≤40B for RTX 5090, at 4-bit) lag the frontier by 6–12 months: GPQA 7.4 mo, MMLU-Pro 7.3 mo, LMArena 12.4 mo, AA index 6.3 mo. Epoch notes that "small open models are more likely to be optimized for specific benchmarks, so the 'real-world' lag may be somewhat longer." Published 2025-08-15 **[older]** — [Epoch AI](https://epoch.ai/data-insights/consumer-gpu-model-gap)
- Open models are 8–10 months behind on private benchmarks and 4–6 months behind on public ones (17 benchmarks). The author says "the gap on real-world tasks is probably even larger" and that open developers are "not fully filtering out benchmark data and training to the test" (speculation). Published 2026-05-28 — [Ihle, LessWrong](https://www.lesswrong.com/posts/rJcCrXyEsJKmmDpWG/how-far-behind-are-open-models)
- An aggregator claims Claude Opus 5 at 96% on SWE-bench Verified (2026-09-22) **[aggregator, unverified]** — [BenchLM](https://benchlm.ai/benchmarks/swe-bench-verified)

### Inferences
- **Single-shot vs agentic spread.** In the Qwen3.6 card, LiveCodeBench v6 spans only 74.6–80.7 across five models whose SWE-bench Verified scores span 17.4–75.0. Gemma4-26B-A4B scores 77.1 on LCB but 17.4 on SWE-bench Verified. Single-shot code generation says little about agentic execution ability.
- **The gap widens with task length.** Qwen3-Coder-Next vs Opus 4.5, same vendor-run table:
  - ~7 points on SWE-bench Verified
  - ~9–12 on SWE-bench Pro
  - ~21–24 on Terminal-Bench 2.0
- **METR ratio.** In the same week, METR measured gpt-oss-120b's horizon at about 1/2.9 of GPT-5's. Kimi K2 Thinking measured about 1/4.3 of Opus 4.5's (Nov 2025). No local-class model has been measured on METR's current suite, which frontier models now hit at 6–17+ hours.
- **Vendor numbers overstate by 12–36 points.** For gpt-oss-120b, Devstral Small 2 and Qwen3-Coder-480B, the vendor/own-harness score exceeds the same-harness score by 12 to 36 points. Duet runs its own harness, so the defensible expectation is the same-harness number, not the model card.
- **Local models spend more steps and more tokens.** Same-harness call counts are 2–3× higher for open models:
  - Devstral Small 86.9 and DeepSeek V3.2 88.5 calls/instance
  - vs Claude 4.6 Opus 28.9 and GPT 5.2 Codex 28.1
  - More calls means more turns and more prefill per task. This bears directly on duet's per-turn prefill cost.
- **Long runs cost many tokens even on the frontier.** Per TB 4.0 trial:
  - Opus 5 xhigh: ~20.9M tokens and ~$18.44
  - GLM-5.3 max: ~26.3M tokens and ~$8.27
  - GPT-6 Astra max: ~4.6M tokens and ~$9.90 (assuming 330 trials, which I did not confirm for this entry)
  - Duet's 1.9–4.8M tokens per run is within the normal range for long agentic tasks, even on the frontier.

### Gaps
- No independent, same-harness SWE-bench/TB results exist for Qwen3.5/3.6-35B-A3B, Qwen3.5-27B or Qwen3-Coder-Next. The SWE-bench team's newest entries are from Feb 2026.
- The official Terminal-Bench 2.0 leaderboard could not be retrieved (tbench.ai now serves TB 4.0), so TB2.0 frontier numbers are vendor or aggregator figures.
- No METR time horizon exists for any model that fits in 32–128 GB after gpt-oss-120b (Aug 2025).
- No study found that measures accuracy at the Q4/Q6 quantization people actually run locally. Vendor scores come from BF16/FP8 serving.
- The duet assumption that frontier planning plus review compensates for a weaker executor was not tested by any source in this scope.

## Q2. Is there evidence that small/local models are specifically weak at agentic behaviors (tool-call reliability, multi-step instruction following, long-context use, knowing when to stop, looping/repeated reads)?

### Takeaway
Yes, but mostly as secondary signals: failure-mode taxonomies, step counts, edit-format compliance and harness sensitivity. No clean benchmark isolates each behavior for 30B-class models against the frontier. The strongest evidence:
- Qwen3 32B has the highest tool-error share on SWE-Bench Pro.
- Small models have lower edit-format compliance on Aider.
- Open models take 2–3× more steps per SWE-bench instance.
- Open-model scores swing dramatically across harnesses.
- Long-context degradation affects every model, frontier included.

### Cited Findings
- **Failure taxonomy (SWE-Bench Pro, Nov 2025)** **[older]** — [SWE-Bench Pro](https://arxiv.org/html/2509.16941v2):
  - Qwen3 32B "exhibits the highest tool error rate (42.0%)" among failed trajectories.
  - Claude Opus 4.1 fails mainly on semantics: wrong solutions 35.9%, syntax 24.2%.
  - Claude Sonnet 4's main failure is context overflow (35.6%), with "endless file reading" at 17.0%.
  - Gemini 2.5: tool errors 38.8%, syntax 30.5%, wrong solution 18.0%.
  - Looping/endless-reading is therefore not unique to small models.
- **Edit-format compliance (Aider Polyglot)** **[older]**: gpt-oss-120b 79.1% and Qwen3 32B 83.6%, vs 97–99.6% for Claude Opus 4 (97.3%), Gemini 2.5 Pro (99.6%) and DeepSeek V3.2 (97.3–98.2%) — [Aider](https://aider.chat/docs/leaderboards/)
- **Step counts (same harness, mini-SWE-agent)**: Devstral Small 2512 86.9, DeepSeek V3.2 88.5, GLM 5 76.2 and MiniMax M2.5 60.5 calls/instance, vs Claude 4.6 Opus 28.9, Claude 4.5 Opus 32.9, GPT 5.2 Codex 28.1 — [SWE-bench data](https://github.com/SWE-bench/swe-bench.github.io/blob/master/data/leaderboards.json)
- **Harness/tool-format sensitivity** — [Qwen3-Coder-Next report](https://arxiv.org/html/2603.00729v1):
  - Kimi-K2.5 on TB2.0: 49.4 under Terminus2-json vs 0.9 under Claude Code.
  - Qwen3-Coder-Next: 36.2 vs 25.8 across harnesses.
  - Qwen3-Coder-Next averages 92.7% across five IDE/CLI scaffolds, but only 83.0% on one of them.
  - Its RL training raised average agent turns "from 50 to 130".
  - It needed a "reward hacking blocker" to stop agents from using git to retrieve ground truth.
- **Turn/budget caps hurt weaker models most.** Under a 50-turn/$2 cap, Qwen-3 32B resolved 3.4% of SWE-Bench Pro vs GPT-5 (medium) 23.3% **[older]** — [SWE-Bench Pro](https://arxiv.org/html/2509.16941v2)
- **Long context ("context rot", Chroma, July 2025)** **[older]**: all 18 models tested lost reliability as input length grew, even on simple retrieval/replication. Models included GPT-4.1, Claude 4, Gemini 2.5 and Qwen3 — [Chroma](https://www.trychroma.com/research/context-rot)
- **Tool/MCP benchmarks for 2026 local models (vendor, no frontier comparator in the same table)**: TAU3-Bench 67.2, MCPMark 37.0, MCP-Atlas 62.8 for Qwen3.6-35B-A3B. Qwen3.5-27B scores 68.4 / 36.3 / 68.4 — [Qwen3.6 card](https://huggingface.co/Qwen/Qwen3.6-35B-A3B)
- **BFCL v4 multi-turn**: the only small-model figures I found were a secondary report: Qwen3-4B 16.88% and Qwen3-1.7B 8.38% multi-turn with prompt-based function calling, and the claim that multi-turn scores drop 5–10 points vs single-turn for every model **[secondary, unverified]** — [Spheron blog](https://www.spheron.network/blog/tool-calling-benchmarks-bfcl-tau-bench-latency-optimization/)
- **Pass@1 hides behavior.** An analysis of 138,000 agent trajectories finds pass@1 "relatively even across frontier models" while edit frequency, testing activity and phase transitions differ by model (2026-06-16). It gives no loop metrics by open vs closed — [Gupta et al.](https://arxiv.org/abs/2606.17454)

### Inferences
- The failure modes duet hit match these taxonomies: repeated reads, not stopping, and tool-format errors. They are amplified for small models (tool errors, format compliance, step counts), but they also appear in frontier models (Sonnet 4's endless reading).
- Harness sensitivity is large for open models. Duet's own tool schema and prompt format could move a local model's score by tens of points either way, so any local-vs-frontier comparison must be run in duet's harness.
- Open models take 2–3× more calls. If a local executor takes 2–3× the turns of a frontier executor, total prefill grows superlinearly: more turns, each over a longer context.

### Gaps
- No public benchmark isolates "knowing when to stop" or repeated-read loops per model with 30B-class vs frontier comparisons.
- Per-model BFCL v4 multi-turn data from the official leaderboard was not retrieved.
- No study found of how quantization (Q4 vs BF16) affects tool-call validity rates.

## Q3. Local inference throughput on Apple Silicon (MLX, llama.cpp, Ollama, LM Studio): prefill vs decode for 30B-class MoE and dense models at 16K–64K, how prefill slows with context, how prefix/KV caching works and when it is invalidated, and how this compares with cloud time-to-first-token.

### Takeaway
On M-series Max/Ultra, prefill is compute-bound and quantization does not speed it up:
- 3–4B-active MoEs run at roughly 1,000–1,850 tok/s.
- ~32B dense models run at roughly 150–350 tok/s.
- Prefill drops about 20–40% by 32K and about 55% by 128K.

Decode is bandwidth-bound and also degrades with depth. Prefix caching works only when the client keeps the prompt prefix byte-identical. On hybrid/SWA models (Qwen3.5/3.6, Qwen3-Coder-Next, Gemma 4), llama.cpp and MLX servers frequently fell back to full re-prefill in 2026. Cloud frontier prefill is 2–3 orders of magnitude faster per token.

### Cited Findings
**Prefill vs depth on Apple Silicon:**
- **M4 Max** (MacBook Pro 64 GB), llama.cpp build 7030, gpt-oss-20b MXFP4 (20.9B total / 3.6B active), 2025-11-16 **[older]**:
  - pp2048 1,849.79 tok/s
  - pp8192 1,613.83
  - pp16384 1,388.79
  - pp32768 1,093.58
  - tg128 117.83 tok/s
  - DGX Spark on the same model: pp2048 3,797.76 → pp32768 3,094.77, tg128 86.23.
  - Source: [llama.cpp discussion #16578](https://github.com/ggml-org/llama.cpp/discussions/16578)
- **M3 Ultra 512 GB**, MLX, 276 runs, 2026-03-05 — [MLX discussion #3209](https://github.com/ml-explore/mlx/discussions/3209):
  - Qwen 32B (dense) prompt throughput: ~345 tok/s @1K, ~333 @8K, ~271 @32K (−21%), ~154 @128K (−55%).
  - Qwen 32B Q4 decode: 31.2 tok/s @1K, 23.9 @16K, 19.0 @32K, 13.4 @64K, 8.5 @128K.
  - TTFT @32K: 121 s for Qwen 32B Q4 vs 55 s for Mixtral 8x7B Q4 (12.9B active).
  - Llama 405B TTFT at 16K: ~10.3 min regardless of quantization.
  - Rule of thumb given: TTFT ≈ 2 × params × tokens / TFLOPS, with the M3 Ultra at ~54 TFLOPS FP16. "Quantization does not help" prefill.
  - At 128K, Q2 decode is only 1.7× F16, because the FP16 KV cache dominates bandwidth.
- **M5 (base)**, MacBook Pro 24 GB, MLX, 4,096-token prompt (Apple, 2025-11-19) **[older]**:
  - TTFT under 3 s for a 30B MoE (Qwen 30B-A3B 4-bit) and under 10 s for a 14B dense model.
  - M5 vs M4: TTFT 3.33–4.06× faster (neural accelerators); decode only 1.19–1.27× faster.
  - Bandwidth: 153 vs 120 GB/s.
  - Apple: "Generating the first token is compute-bound… subsequent tokens [are] bounded by memory bandwidth."
  - Source: [Apple ML Research](https://machinelearning.apple.com/research/exploring-llms-mlx-m5)
- **M5 Max 128 GB**, llama.cpp, 32K-context prefill (2026-03-31): Llama-3.1-70B Q4_K_M 75.2 tok/s (q8_0 KV); Command-R+ 104B 62.3 tok/s. A 104B pass at 128K took 4,996 s — [llama.cpp discussion #20969](https://github.com/ggml-org/llama.cpp/discussions/20969)
- **Decode vs depth**, Gemma4 26B-A4B Q4_K_M on M4 Pro 48 GB, llama.cpp, f16 KV (2026-04-04): pp512 770 tok/s; tg128 60.8 @d512, 51.9 @d16K, 45.8 @d32K, 36.0 @d65K, 21.1 @d131K — [llama.cpp discussion #20969](https://github.com/ggml-org/llama.cpp/discussions/20969)
- **Current Apple memory bandwidth** (spec page, 2026-09-22): M5 Max 460 GB/s (32-core GPU) or 614 GB/s (40-core GPU), up to 128 GB. M5 Ultra 1.2 TB/s, up to 512 GB — [Apple Mac Studio specs](https://www.apple.com/mac-studio/specs/)

**Prefix/KV caching and invalidation:**
- **llama.cpp server** reuses only the common token prefix of the slot's previous prompt. Anything that changes early tokens forces reprocessing from that point. Maintainers attributed "full re-processing" reports to clients mutating the prefix — [llama.cpp #19794](https://github.com/ggml-org/llama.cpp/issues/19794), [#20225](https://github.com/ggml-org/llama.cpp/issues/20225), [#20153](https://github.com/ggml-org/llama.cpp/issues/20153):
  - "OpenCode mutates the prefix" (ggerganov, 2026-03-08).
  - A log showing only 3,069 of 19,906 tokens reusable "suggests a changing system message" such as an embedded current time (2026-02-22).
  - A user reported Claude Code needed `CLAUDE_CODE_ATTRIBUTION_HEADER=0` to keep the prefix stable (2026-02-24).
  - The server "does not cache prompts that are less than 512 tokens long" (2026-03-09).
- **Hybrid/recurrent models can't rewind the KV cache to arbitrary positions** — [#20225](https://github.com/ggml-org/llama.cpp/issues/20225), [#19794](https://github.com/ggml-org/llama.cpp/issues/19794), [#21831](https://github.com/ggml-org/llama.cpp/issues/21831):
  - Qwen3.5/3.6 (Gated DeltaNet), Qwen3-Coder-Next and Nemotron 3 Nano log "forcing full prompt re-processing due to lack of cache data (likely due to SWA or hybrid/recurrent memory)" unless a context checkpoint exists at the divergence point.
  - Reported impact: "a 15k-token conversation takes ~8 minutes per turn instead of seconds" (Qwen3.5-27B Q8_0, AMD R9700, 2026-03-08).
  - Workaround reported to fix Qwen3.5/3.6-35B-A3B, including on Apple: `--checkpoint-every-n-tokens 1024 --ctx-checkpoints 256` (2026-04-16/24).
  - Issue #21831 was still open as of 2026-09-22.
  - GLM-4.7-Flash (pure attention) did not show the problem in the same client.
- **MLX:**
  - mlx-lm issue title: "Prefix cache reuse is broken for all hybrid-architecture models (sliding window, SSM/Mamba)" — [mlx-lm #980](https://github.com/ml-explore/mlx-lm/issues/980).
  - Per-request `chat_template_kwargs` bypass the mlx_lm.server prompt cache (v0.31.3) — [mlx-lm #1803](https://github.com/ml-explore/mlx-lm/issues/1803).
  - mlx-vlm server clears the Metal cache after every request, "forcing full re-prefill on every turn" — [mlx-vlm #999](https://github.com/Blaizzy/mlx-vlm/issues/999).
- **LM Studio MLX engine v1.8.5** (2026-06-05) — [LM Studio blog](https://lmstudio.ai/blog/mlx-engine-agentic-workloads):
  - Adds disk-backed KV checkpoints at 256-token boundaries, because Qwen 3.5/3.6 (hybrid) and Gemma 4 (SWA) caches are "not arbitrarily rewindable".
  - On an M3 Max 36 GB with Qwen3.6-27B 4-bit: parallel chat 19.42 → 43.33 tok/s; a repeated image prompt went from 23.79 s to 6.88 s.
- **KV-cache quantization is not free.** Symmetric 4-bit KV destroyed Qwen2.5-7B perplexity (4,126 vs 8.03). Asymmetric q8_0-K / 4-bit-V was near-lossless (2026-04-03) — [llama.cpp discussion #20969](https://github.com/ggml-org/llama.cpp/discussions/20969)

**Cloud comparison:**
- Epoch measured marginal latency per extra 10K prompt tokens with prompt caching on (2026-09-08) — [Epoch AI](https://epoch.ai/publications/long-context-latency-scaling-gpt-vs-claude):
  - Claude Sonnet 5: ~0.129 s, roughly constant up to ~1M context.
  - GPT-5.6 Terra: 0.071 s at 100K context, rising to 0.216 s at 1M (quadratic component).
- Artificial Analysis end-to-end TTFT, which includes reasoning time: Claude Opus 5 (high) 16.08 s; GPT-6 Astra (medium) 6.19 s; Opus 5 (max) ~49–63 s. These came from search snippets **[aggregator snippets]** — [AA Opus 5 high](https://artificialanalysis.ai/models/claude-opus-5-high), [AA comparison](https://artificialanalysis.ai/models/comparisons/gpt-6-astra-medium-vs-claude-opus-5)

### Inferences
- **Duet's 117 uncached tok/s is low for its class.** It is roughly 10× below what a 3–4B-active MoE gets in llama.cpp on an M4 Max at 8–32K (1,094–1,614 tok/s). It is below even a dense 32B on an M3 Ultra at 32K (~271 tok/s). Possible explanations, none verified:
  1. The executor is a dense ~30B model or runs on a smaller chip.
  2. Prompts are processed at very deep context.
  3. Framework overhead.
  4. The hybrid-model re-prefill fallback is re-processing more than the ~7K "new" tokens.
- **Per-turn time.** 7,000 tokens at 117 tok/s is ~60 s of prefill per turn. At 1,100–1,600 tok/s it would be ~4.4–6.4 s. A full 32K re-prefill at ~1,094 tok/s costs ~30 s; at 271 tok/s (dense 32B) it costs ~2 minutes.
- **Local vs cloud prefill speed.** 10K new tokens take ~0.13 s of marginal latency on Claude Sonnet 5, vs ~6–9 s on an M4 Max 3.6B-active MoE and ~85 s at duet's measured 117 tok/s. That is roughly 50–650× slower locally for the prefill that dominates duet's wall clock. This mixes a marginal-latency measure with a throughput measure, so treat it as order-of-magnitude only.
- **Cache discipline decides local viability.** Local agent speed depends more on keeping the prefix stable than on the model: no timestamps, stable tool-schema order, no per-turn system-prompt edits, append-only history. Choosing a pure-attention architecture, or configuring checkpoints for hybrid ones, matters as much as raw tok/s.

### Gaps
- I found no published MLX or llama.cpp benchmark of Qwen3-Coder-30B-A3B or Qwen3.6-35B-A3B prefill specifically at 16K/32K/64K on M-series Max/Ultra. The gpt-oss-20b M4 Max series is the closest proxy.
- Ollama-specific prompt-cache behavior (keep_alive, num_ctx default truncation) was not researched in primary docs.
- The cloud TTFT figures from Artificial Analysis were read from snippets, not full pages. They include reasoning time and are not pure prefill.

## Q4. Economics: hardware amortization and electricity for local inference vs 2026 API prices (frontier and cheap cloud, with caching discounts) and flat-rate subscriptions. When the frontier side is on a flat-rate plan, what is the marginal saving from moving tokens to local models?

### Takeaway
At duet's measured 117 tok/s, local electricity costs about $0.09–0.12 per million prefilled tokens. That is about the same as cheap-cloud prices for the same open model ($0.05–0.15/M input) and below frontier cache-read prices ($0.20–0.50/M). Hardware amortization ($105–153/month for a new Mac Studio over 3 years) and hours of wall clock dominate the true cost. When the frontier side is already on a flat-rate plan, the marginal saving from offloading tokens is zero until a plan limit binds. After that it is capped at the price step to the next tier (e.g., $100/month from Claude Max 5x to 20x).

### Cited Findings
**Frontier API prices** (per million tokens, observed 2026-09-22) — [Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing):

| Model | Input | Cache read | Output |
|---|---|---|---|
| Claude Opus 5.5 | $4 | $0.20 (0.05×) | $20 |
| Claude Opus 5 / 4.8 / 4.7 / 4.6 / 4.5 | $5 | $0.50 | $25 |
| Claude Fable 5.1 | $10 | $0.25 (0.025×) | $50 |
| Claude Sonnet 5 | $2 | $0.20 | $10 |
| Claude Haiku 4.5 | $1 | $0.10 | $5 |

- Cache writes cost 1.25× base (5-minute TTL) or 2× base (1-hour TTL). Batch is −50%.
- The 1M context window is billed at standard rates on 4.6+.
- Tool use adds a system prompt of 286–675 tokens depending on model, the bash tool adds 244–325 tokens, and the text editor tool adds 700.
- Claude 4.7+ uses a new tokenizer that "produces approximately 30% more tokens for the same text".

OpenAI (short-context standard tier; Batch/Flex −50%) — [OpenAI pricing](https://developers.openai.com/api/docs/pricing):

| Model | Input | Cached input | Output |
|---|---|---|---|
| gpt-6-astra | $10 | $1.00 | $50 |
| gpt-6-sol | $2 | $0.20 | $10 |
| gpt-6-luna | $0.10 | $0.01 | $0.50 |
| gpt-5.3-codex | $1.75 | $0.175 | $14 |
| gpt-5.6-sol | $4 | $0.40 | $20 |

- Long context costs 2× input.

Z.ai direct — [Z.ai pricing](https://docs.z.ai/guides/overview/pricing):

| Model | Input | Cached | Output |
|---|---|---|---|
| GLM-5.3 | $1.40 | $0.26 | $4.40 |
| GLM-4.7 | $0.60 | $0.11 | $2.20 |
| GLM-4.5-Air | $0.20 | $0.03 | $1.10 |
| GLM-4.7-Flash / 4.5-Flash | free | free | free |

**Cheap-cloud prices for local-class open models** (OpenRouter API, 2026-09-22; input / output / cache-read per million) — [OpenRouter models API](https://openrouter.ai/api/v1/models):

| Model | Input | Output | Cache read |
|---|---|---|---|
| qwen3-coder-30b-a3b-instruct | $0.07 | $0.28 | — |
| qwen3.6-35b-a3b | $0.15 | $1.00 | $0.05 |
| qwen3.6-27b | $0.32 | $2.70 | $0.15 |
| qwen3-coder-next | $0.12 | $0.80 | $0.07 |
| gpt-oss-20b | $0.018 | $0.09 | — |
| gpt-oss-120b | $0.15 | $0.60 | $0.075 |
| devstral-2512 | $0.40 | $2.00 | $0.04 |
| nemotron-3-nano-30b-a3b | $0.05 | $0.20 | $0.03 |
| glm-4.7-flash | $0.061 | $0.40 | — |
| gemma-4-26b-a4b-it | $0.09 | $0.30 | $0.05 |

**Flat-rate subscriptions:**
- Claude: Pro $20/month ($17 annual). Max 5x $100/month and Max 20x $200/month (web prices). Limits reset every 5 hours, plus a weekly cap across all models. Claude Code is included — [Claude Max help](https://support.claude.com/en/articles/11049741-what-is-the-max-plan), [claude.com/pricing](https://claude.com/pricing)
- ChatGPT/Codex — [ChatGPT pricing](https://learn.chatgpt.com/docs/pricing):
  - Go $8, Plus $20, Pro $100 (5×) or Pro 20× (secondary sources list it at $200).
  - Plus gives Codex 5–45 GPT-6 Astra, 15–150 Sol or 350–3,000 Luna messages per 5 hours.
  - Credits are priced per million tokens: Astra 250 in / 1,250 out; Sol 50/250; Luna 2.5/12.5.
  - Secondary reports say credits cost $0.04 each and that the 5-hour limit was temporarily removed on 2026-07-12 **[secondary]** — [Morph](https://www.morphllm.com/codex-pricing), [SimpleMetrics](https://simplemetrics.xyz/chatgpt-codex-limits-2026/)
- Z.ai GLM Coding Plan, list prices Lite $18, Pro $72, Max $160/month. Discounts are 10% monthly, 20% quarterly, 30% yearly. Quotas are ~80 / 400 / 1,600 prompts per 5 hours and ~400 / 2,000 / 8,000 per week **[aggregator; official devpack page not fetched]** — [AI Pricing Guru](https://www.aipricing.guru/z-ai-subscription-pricing/), [HyScaler](https://hyscaler.com/insights/glm-coding-plan-review/)

**Hardware and power:**
- Current Mac Studio prices (Apple Store, 2026-09-22): M5 Max 36 GB/512 GB $2,499; M5 Max (40-core GPU) 64 GB/1 TB $3,799; M5 Ultra 96 GB/1 TB $5,499. Upgrade prices for 128 GB and 256/512 GB were not captured — [Apple Store](https://www.apple.com/shop/buy-mac/mac-studio)
- Apple's maximum power figures ("compute-intensive test application"; idle in parentheses): M4 Max 145 W (6 W), M3 Ultra 270 W (9 W), M5 Max 200 W (7 W), M5 Ultra 385 W (9 W). The M5 Ultra power supply's maximum continuous rating is 480 W — [Apple Support 102027](https://support.apple.com/en-us/102027), [Apple specs](https://www.apple.com/mac-studio/specs/)
- A May 2026 benchmark study reports the M3 Ultra delivers ~23× the tokens per joule of an RTX 5090 on Qwen2.5-1.5B. It gives no watt figures — [Silicon Showdown](https://arxiv.org/html/2605.00519v2)
- US average residential electricity: 17.3¢/kWh in 2025, 18.2¢ in 2026 (forecast), 18.6¢ in 2027 (EIA STEO released 2026-09-09) — [EIA](https://www.eia.gov/outlooks/steo/report/elec_coal_renew.php)

### Inferences
All arithmetic below is mine.

**Local electricity per million prefilled tokens at duet's rate**
- 1M tokens ÷ 117 tok/s = 8,547 s = 2.37 h.
- At Apple's maximum draw: M5 Max 200 W → 0.475 kWh → $0.086/M; M3 Ultra 270 W → $0.117/M, at 18.2¢/kWh. Real draw is below the maximum, so these are upper bounds.
- At a 3.6B-active MoE's ~1,100–1,600 tok/s, electricity falls to about $0.006–0.01/M.

**Electricity compared with API prices**
- Same as or slightly below cheap-cloud uncached input for the same class of model ($0.07–0.15/M).
- Above cheap-cloud cache reads ($0.03–0.075/M).
- Below frontier cache reads: Opus 5.5 $0.20, Sonnet 5 $0.20, Opus 5 $0.50.
- About 1–2% of frontier uncached input ($4–10/M).
- Frontier caching discounts (0.025–0.1×) close most of the gap for agent workloads that mostly re-read cached context.

**Amortization dominates**
- $3,799 over 36 months is $105.5/month; $5,499 is $152.8/month.
- At 117 tok/s for 8 h/day, the machine processes ~101M tokens/month → ~$1.04/M amortization. At 24/7 it is ~$0.35/M.
- That exceeds cheap-cloud prices, unless the Mac would be owned anyway. In that case the marginal cost is only electricity and wear.

**Per-run cost for a 1.9–4.8M-token duet run**
- Frontier API with no caching (Opus 5 input $5/M): ~$9.50–24.
- Frontier API, all cache reads: ~$0.95–2.40 (Opus 5) or ~$0.38–0.96 (Opus 5.5 / Sonnet 5 / GPT-6 Sol).
- Cheap cloud Qwen3.6-35B-A3B: ~$0.10–0.72 input side.
- Local electricity: ~$0.15–0.45 for a 4–12-hour run at 200 W.
- The local option is not materially cheaper in dollars than cheap cloud. It is much slower: hours vs minutes, per the Q3 prefill ratios.

**Flat-rate plans**
- If the frontier planner/reviewer is on Claude Max, ChatGPT Pro or the GLM Coding Plan, moving execution tokens to local saves $0 per token while usage stays under the 5-hour and weekly caps.
- The saving only appears when offloading lets the user stay on a cheaper tier (Claude Max 20x → 5x saves $100/month; GLM Max → Pro saves $88/month list) or avoid overage/API billing.
- Against that stand hardware amortization (if bought for this purpose), electricity (~$0.04–0.07/h at max draw) and the wall-clock penalty.
- Execution tokens are also the kind a cheap cloud model (e.g., GLM-4.7-Flash, free on Z.ai; Qwen3.6-35B-A3B at $0.15/M) could handle at cloud speed. So "local" competes with cheap cloud, not only with frontier.

**Token volume is not unusual**
- Frontier agents on TB 4.0 consume ~4.6–26M tokens per trial.
- Execution-side token volume is inherent to long agentic tasks. The cost question comes down to cache-hit rate and $/M after caching.

### Gaps
- No measured wall power for a Mac Studio during sustained long-context prefill was found; only Apple's maximum-load figures.
- Mac Studio 128 GB, 256 GB and 512 GB upgrade prices were not captured. Only base-configuration prices were.
- Official Z.ai GLM Coding Plan terms and the ChatGPT Pro 20× price were confirmed only via secondary sources.
- No public data found on what fraction of Claude Max / ChatGPT Pro users actually hit the caps, which is what the marginal saving depends on.
- Resale value, maintenance and the opportunity cost of tying up a workstation were not modeled.
