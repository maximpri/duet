# Strong/Weak LLM Collaboration: Planner/Executor Splits, Local–Remote Protocols, Routing and Cascades — Empirical Evidence

Scope note (applies to every section): "Measured" = what a source reports. "Inference" = my reading, not the source's claim. Dates are the arXiv submission or post dates. Anything dated before 2025 is flagged **[older evidence]**. Some numbers were pulled from papers through a summarizing fetch tool. When a number could not be checked against a verbatim quote, it is marked "(extracted; verify against table)".

---

## 1. Stanford Hazy Research "Minions" (local–remote collaboration, Feb 2025): quality kept, cost saved, task types, failure modes

### Takeaway
On long-document QA (finance, medical, science papers), the naive chat protocol (Minion) cut cloud cost about 30× and kept about 87% of GPT-4o-only accuracy. The decomposed protocol (MinionS), where the cloud model writes the decomposition and the local model runs many small single-step jobs on chunks, kept about 97.9% at about 5.7× lower cost with an 8B local model. The authors tested no coding, agentic or multi-turn tool-use tasks. The two small-model failure modes they named, multi-step instructions and long contexts, are exactly what a coding agent's per-turn tool loop asks of its executor.

### Cited Findings
- Paper: "Minions: Cost-efficient Collaboration Between On-device and Cloud Language Models", Narayan, Biderman, Eyuboglu, May, Linderman, Zou, Ré (Stanford / Together AI). arXiv 2502.15964v1, **21 Feb 2025**. — [arXiv 2502.15964](https://arxiv.org/html/2502.15964)
- **Minion (naive protocol: local and remote models chat freely; only the local model reads the full context):** about **30.4× average cloud-cost reduction** while keeping **87% of remote-only performance**. Per-dataset cost reductions: 38.13× FinanceBench, 31.3× LongHealth, 20.9× QASPER (extracted; verify against table). — [arXiv 2502.15964](https://arxiv.org/html/2502.15964)
- **MinionS (decomposition protocol):** the remote model writes decomposition *code* without reading the full context. The local model runs the resulting subtasks in parallel over document chunks. The remote model then aggregates and decides whether another round is needed. Verbatim result: "MinionS with an 8B parameter local LM can recover **97.9%** of the performance of remote-only systems at **18.0%** of the cloud cost", averaging a **5.7×** cost reduction. A 3B local model was reported at about 93.4% of performance for about 16.6% of cloud cost (extracted; verify against table). — [arXiv 2502.15964](https://arxiv.org/html/2502.15964)
- **Table 1 accuracies (remote = GPT-4o; local = Llama-8B):**
  - Remote-only: FinanceBench 0.724, LongHealth 0.826, QASPER 0.748
  - Minion: 0.630 / 0.804 / 0.635
  - MinionS: 0.709 / 0.804 / 0.740
  - Local-only: 0.444 / 0.326 / 0.468
  
  Remote-only average cost was $0.233 (FinanceBench), $0.261 (LongHealth) and $0.301 (QASPER). — [arXiv 2502.15964](https://arxiv.org/html/2502.15964)
- **Tasks:** three long-document reasoning and QA benchmarks only: FinanceBench (financial filings, about 142.9K tokens on average), LongHealth (medical records, about 120.1K tokens) and QASPER (scientific papers, about 54K tokens). — [arXiv 2502.15964](https://arxiv.org/html/2502.15964)
- **Local models tested:** Llama-3.2-1B/3B, Llama-3.1-8B-Instruct, Qwen2.5-1.5B/3B/7B-Instruct. **Primary remote model:** GPT-4o. MinionS "starts being competitive" at about the 3B scale. Performance scales steeply with local-model size, and 1B models retained only a small fraction of remote-only quality (extracted; verify exact 1B figures). The authors say this kind of local–remote system became feasible only around mid-2024, with GPT-4-turbo and Llama-3.1-8B. — [arXiv 2502.15964](https://arxiv.org/html/2502.15964)
- **Small-model failure modes named by the authors (these motivated MinionS):**
  1. **Multi-step instructions:** "splitting sub-parts into separate requests leads to a **56 point** performance improvement".
  2. **Long-context confusion:** "increasing context length from <1K to >65K tokens can decrease performance by **13%** on a simple extraction instruction".
  
  — [arXiv 2502.15964](https://arxiv.org/html/2502.15964)
- **Stated limitations:**
  - No latency optimization. The appendix bounds the latency increase at ≤5×.
  - No privacy analysis.
  - Local compute is treated as free, ignoring the "fixed cost of hardware and marginal cost of energy consumption".
  - The models "were trained independently and therefore might not know each other's capabilities".
  - No coding, multi-turn tool invocation or code-execution scenarios were evaluated.
  
  — [arXiv 2502.15964](https://arxiv.org/html/2502.15964)

### Inferences
- The protocol that works (MinionS) gives the local model **short-context, single-step, parallel, stateless extraction jobs**, and the frontier model does all multi-step reasoning and aggregation. The naive protocol, where the local model drives a free conversation over long context, loses about 13% of quality. Of the two, duet's "local model executes every tool-calling turn" is closer to the naive Minion protocol, or worse: each executor turn is multi-step, long-context and stateful, and depends on earlier turns.
- The 97.9% / 5.7× figure should not be carried over to coding. Minions tasks are read-only QA, where the local model's output is evidence for the remote model to aggregate. In a creation task, the local model's output *is* the artifact.
- The authors' own identified failure modes (multi-step instruction following, long-context degradation) predict that a local executor running a multi-turn coding loop will do worse than in Minions.

### Gaps
- No Minions result on coding, agents or tool use. The paper explicitly does not test this. I found no 2025–2026 Minions follow-up applying the protocol to SWE-bench-style or creation tasks.
- The exact per-model 1B/3B retention numbers were not verified verbatim.

---

## 2. Aider "architect/editor" mode (Sept 2024 onward): pairs, scores, and whether the editor was weak

### Takeaway
Aider's architect/editor gains came from pairing a strong reasoning architect with an editor that was itself a **strong or near-frontier coder**: o1-mini, DeepSeek V2.5 or Claude 3.5 Sonnet. The editor only converts a described change into file edits in a single pass. It runs no tool loop. Several pairings scored *below* the architect model running solo or homogeneously. Aider later reported that using other models as editor did not improve o1 or R1. The only small/local-model data point is a pair where both architect and editor were local 32B models. I found no frontier-architect + small-local-editor result.

### Cited Findings
- **Post: "Separating code reasoning and editing", aider.chat, 26 Sep 2024 [older evidence].** Benchmark: aider's code-editing benchmark. — [aider architect post](https://aider.chat/2024/09/26/architect.html)
- **Pair results (architect → editor, edit format, pass rate):**
  - o1-preview → o1-mini (whole): **85.0%**
  - o1-preview → DeepSeek (whole): **85.0%**
  - o1-preview → Claude 3.5 Sonnet (diff): 82.7%
  - o1-preview → DeepSeek (diff): 80.5%
  - o1-preview → GPT-4o (diff): 80.5%
  - Sonnet → Sonnet (diff): 80.5%
  - Sonnet → DeepSeek (diff and whole): 78.9%
  - GPT-4o → GPT-4o (diff): 75.2%
  - GPT-4o → DeepSeek (diff): 74.4%; (whole): 73.7%
  - o1-mini → DeepSeek (whole): 71.4%; (diff): 69.2%
  - o1-mini → GPT-4o (diff): 70.7%
  - GPT-4o-mini → GPT-4o-mini (whole): 60.2%
  
  — [aider architect post](https://aider.chat/2024/09/26/architect.html)
- **Solo baselines in the same post:** o1-preview 79.7%, Claude 3.5 Sonnet 77.4%, GPT-4o 71.4%, o1-mini 61.1% (all diff); GPT-4o-mini 55.6% (whole). — [aider architect post](https://aider.chat/2024/09/26/architect.html)
- Aider's observations:
  - "Deepseek is surprisingly effective as an Editor model". It "helps all the Architect models except for Sonnet".
  - The top whole-format results make "both of these steps … quite slow, so probably not practical for interactive use".
  
  The "DeepSeek" editor here is DeepSeek's 2024 API model, a large open-weight MoE, not a small local model. — [aider architect post](https://aider.chat/2024/09/26/architect.html)
- **Pairs that scored below a simpler configuration (measured):**
  - Sonnet → DeepSeek 78.9% vs Sonnet → Sonnet 80.5%
  - GPT-4o → DeepSeek 74.4% vs GPT-4o → GPT-4o 75.2%
  - o1-mini → GPT-4o 70.7% vs GPT-4o solo 71.4%
  
  — [aider architect post](https://aider.chat/2024/09/26/architect.html)
- **Local-model pair: "QwQ is a code architect, not an editor", aider.chat, 3 Dec 2024 [older evidence].** Code-editing benchmark:
  - QwQ-32B alone: 42.1% (edit-format compliance 91.0%)
  - Qwen2.5-Coder-32B-Instruct alone: 71.4%
  - QwQ → Qwen2.5-Coder-32B: **73.6%** (100% format compliance)
  - QwQ → Claude 3.5 Haiku: 71.4%
  - QwQ → DeepSeek V2.5: 67.7%
  - o1-preview solo: 79.7%
  
  Aider notes that QwQ "was unable to comply with even the simplest editing format" solo, and that the QwQ+Qwen pair "is well below the SOTA results". — [aider QwQ post](https://aider.chat/2024/12/03/qwq.html)
- **"R1+Sonnet set SOTA on aider's polyglot benchmark", aider.chat, 24 Jan 2025.**
  - R1 (architect) + Claude 3.5 Sonnet (editor): **64.0%** on the polyglot benchmark, for **$13.29**. This was "14X less cost compared to the previous o1 SOTA result".
  - Aider also reports that "o1 paired with Sonnet didn't produce better results than just using o1 alone", and that "using various other models as editor didn't seem to improve o1 or R1 versus their solo scores".
  
  — [aider R1+Sonnet post](https://aider.chat/2025/01/24/r1-sonnet.html)

### Inferences
- In every top aider pair the editor was a strong model. The Jan 2025 SOTA pair used Sonnet, a frontier model at the time, as *editor*. The cost savings came from a cheap *architect* (R1) replacing o1, not from a cheap editor. This is the opposite assignment to duet, where the frontier model plans and a cheap local model does the execution.
- Aider's editor job is narrower than duet's executor job: one pass that converts a written change description into file edits, with no search, no tool calls, no test runs and no multi-turn state. Even for this narrower role, aider data shows editor choice can pull a pair *below* the stronger model working alone.
- The QwQ result shows the split helps when the *architect* cannot follow the edit format and the editor can. Rescuing a weak model in the architect role is a different mechanism from rescuing a weak *executor*.

### Gaps
- I found no aider benchmark with a frontier architect (o1, Sonnet, GPT-4o, etc.) and a **small local editor (≤32B)**, and no aider data where the editor must run a multi-turn tool loop.
- The Sept 2024 benchmark is aider's older code-editing benchmark and the Jan 2025 result uses polyglot, so scores are not comparable across posts.

---

## 3. Model routing and cascades (RouteLLM, FrugalGPT, AutoMix, Hybrid LLM, 2025–2026 agent routing): cost vs quality, and transfer to multi-turn agents

### Takeaway
The classic routing and cascade results report large cost savings: FrugalGPT up to 98%, RouteLLM more than 85% on MT-Bench at 95% of GPT-4 quality, AutoMix more than 50%, Hybrid LLM up to 40% fewer large-model calls. All of them decide once per independent query, on chat, QA or math benchmarks, and savings shrink on harder benchmarks (45% on MMLU, 35% on GSM8K for RouteLLM). The 2026 agent-routing papers that do route inside multi-turn episodes differ from duet in two ways:
- They route among capable models, not a frontier model and a small local one, or escalate by **restarting** the strong model rather than continuing from the weak model's trajectory.
- They report that switching mid-trajectory has costs: bias toward the weak model's mistakes, and lost KV-cache reuse.

### Cited Findings
- **RouteLLM** (Ong, Almahairi, Wu, Chiang, Wu, Gonzalez, Kadous, Stoica), arXiv 2406.18665, submitted **26 Jun 2024**, v4 23 Feb 2025 **[older evidence]**. Abstract claim: costs reduced "by over 2 times in certain cases" "without compromising the quality of responses". Routers transfer "even when the strong and weak models are changed at test time". — [arXiv 2406.18665](https://arxiv.org/abs/2406.18665)
- **RouteLLM blog (LMSYS, 1 Jul 2024) [older evidence]:**
  - Strong model GPT-4 Turbo, weak model Mixtral 8x7B.
  - Cost reductions at 95% of GPT-4 performance: **MT-Bench more than 85%, MMLU 45%, GSM8K 35%**.
  - On MT-Bench, the matrix-factorization router needed 26% GPT-4 calls with Arena data only, and **14%** with LLM-judge-augmented data.
  - MMLU results were "poor" with Arena data alone because of distribution mismatch, and needed about 1,500 task-specific samples. GSM8K gains were smaller.
  - Routers generalized to Claude 3 Opus / Llama 3 8B without retraining.
  - RouteLLM claimed the same performance as the Martian and Unify commercial routers at more than 40% lower cost.
  
  — [LMSYS RouteLLM blog](https://lmsys.org/blog/2024-07-01-routellm/)
- **FrugalGPT** (Chen, Zaharia, Zou), arXiv 2305.05176, **9 May 2023 [older evidence]**. An LLM cascade that can "match the performance of the best individual LLM (e.g. GPT-4) with up to **98%** cost reduction", or "improve the accuracy over GPT-4 by 4% with the same cost". Models included GPT-4, ChatGPT and J1-Jumbo. — [arXiv 2305.05176](https://arxiv.org/abs/2305.05176)
- **AutoMix** (Aggarwal, Madaan, et al.), NeurIPS 2024, last revised 19 Jan 2025 **[older evidence]**. The small model generates an answer, a few-shot self-verification scores it, and a POMDP router decides whether to escalate. It reduces "computational cost by over **50%** for comparable performance" on five datasets with five LMs. — [arXiv 2310.12963](https://arxiv.org/abs/2310.12963)
- **Hybrid LLM** (Ding, Mallick, Wang, et al., Microsoft), ICLR 2024, arXiv 2404.14618 **[older evidence]**. A router based on predicted query difficulty achieves "up to **40%** fewer calls to the large model, with no drop in response quality". — [arXiv 2404.14618](https://arxiv.org/abs/2404.14618)
- **MTRouter: cost-aware multi-turn routing** (Zhang, Li, Wang, et al.), arXiv 2604.23530, **26 Apr 2026**.
  - Turn-level routing inside agent episodes on **ScienceWorld** and **HLE with web tools**.
  - Six-model pool: GPT-5, DeepSeek-V3.2, MiniMax-M2, Kimi-K2, Gemini-2.5-Flash-Lite, GPT-OSS-120B.
  - Against the GPT-5-only baseline: ScienceWorld **48.4 → 53.8 (+5.4) at 58.7% lower cost**; HLE **25.1% → 26.0% at 43.4% lower cost**.
  - Motivation: episodes mix steps "from high-stakes strategic planning to routine tool invocations", so one model choice per episode cannot adapt.
  - Behavior: MTRouter switches **fewer** times than the Router-R1 baseline (about 5 vs about 20 switches on ScienceWorld). After an error it keeps the current model about 90% of the time, vs Router-R1's 38.3%.
  - Stated limitations: expensive trajectory collection (episodes capped at 50 and 30 steps), offline-only learning, and "frequent switching can reduce KV cache hits".
  
  — [arXiv 2604.23530](https://arxiv.org/html/2604.23530)
- **SWE-Router: routing in multi-turn agentic SWE tasks** (Son, Yoon, Tang, Wang, Wolf, Bogunovic), arXiv 2607.00053, **30 Jun 2026**.
  - Setup: the weak model (gpt-5-mini or deepseek-v3.2) runs the first K ≤ 4 turns of mini-SWE-agent on **SWE-bench Verified**. A Qwen2.5-Coder-7B value head then decides whether to escalate to the strong model (gemini-3-pro-preview).
  - On escalation, the strong model "*restarts* from q: continuation rules need expensive online m2 inference, and **conditioning m2 on m1's reasoning has been seen to bias m2 toward m1's mistakes**."
  - Rationale for the split: "SWE agents typically spend their early turns locating files and isolating failures — exploration cheaper than patch synthesis".
  - Route-AUC improvements are +12 pp (gpt-5-mini pair) and +15.3 pp (deepseek-v3.2 pair) over baseline. The routing curve briefly passes *above* all-strong, because "weak and strong solve non-identical instance subsets".
  - A task description alone often cannot tell "a localized typo" from "a multi-module refactor". Early-turn observations supply that signal.
  
  — [arXiv 2607.00053](https://arxiv.org/html/2607.00053)

### Inferences
- The routing literature's headline savings come from **per-query** decisions on MT-Bench, MMLU, GSM8K and QA, where queries are independent and a wrong weak answer costs only that one query. They do not transfer directly to a multi-turn tool loop, where a weak turn changes the state (files, context) that every later turn inherits.
- The two 2026 multi-turn routers point *against* duet's design:
  - SWE-Router gives the weak model only the **early exploration turns**, never patch synthesis. It **discards** the weak trajectory on escalation, citing contamination of the strong model by the weak model's mistakes.
  - MTRouter's success comes with *fewer* switches and high stickiness to one model.
  - Duet assigns *every* execution turn, including synthesis and edits, to the weak model, and the frontier reviewer reasons over the weak model's trajectory. SWE-Router's authors warn about exactly this conditioning pattern.
- MTRouter's "weak" models (DeepSeek-V3.2, Kimi-K2, GPT-OSS-120B) are near-frontier, not small local models. Its gains cannot be read as evidence that a small local model can hold execution turns.

### Gaps
- I did not extract SWE-Router's exact resolve rates (all-weak, all-strong, router operating point) or its cost fractions. The HTML reports them mainly as figures and Route-AUC.
- I found no routing paper that measures a frontier model paired with a ≤32B **local** model inside a multi-turn coding agent loop.
- I did not collect RouteLLM's per-benchmark APGR/CPT tables beyond the blog summary.

---

## 4. Planner/executor splits in agents (Plan-and-Act, ReWOO, α-UMi, LLMCompiler, D-CIPHER, PlanAhead, plan transfer, advisors): is the executor the binding constraint?

### Takeaway
The one study I found that holds a strong planner fixed and swaps in a weaker **tool-calling executor** in a long-horizon agentic domain is D-CIPHER (CTF / offensive security, 2025). It reports "consistent under-performance with weaker models": Sonnet planner + Haiku executor solved 13.0% vs 19.0% for Sonnet in both roles. The authors conclude that both roles "require stronger models". Studies where planning dominates, such as Plan-and-Act, used equally capable, fine-tuned executors on short web actions. In 2026 SWE-bench work, a weak proposer's coverage limits what stronger selection or review can recover.

### Cited Findings
- **D-CIPHER** (Udeshi et al.), arXiv 2502.10931, Feb 2025 (revised May 2025). This is a multi-agent planner plus heterogeneous executors for CTF challenges.
  - Headline results: **NYU CTF Bench 22.0%**, **Cybench 22.5%**, **HackTheBox 44.0%**.
  - Homogeneous baselines (Table III, NYU CTF Bench): Claude 3.5 Sonnet in all roles **19.0%**; GPT-4o in all roles **10.5%**.
  - Strong planner + weaker executor (Table IV, NYU CTF Bench):
    - Sonnet → Claude 3.5 Haiku: **13.0%** ($0.33 per challenge)
    - GPT-4o → GPT-4o-mini: **6.5%** ($0.03)
    - GPT-4 Turbo → GPT-4o-mini: 5.5% ($0.07)
    - Gemini 1.5 Flash → Flash-8B: 3.0% ($0.001)
    - Llama 3.1 405B → Llama 3.3 70B: **0.0%**
  - Verbatim: "we experiment by combining stronger models for the Planner with weaker models for the Executor. The results … show[ ] **consistent under-performance with weaker models**", and "Both the Planner and Executor tasks are complex to require stronger models."
  - Stated limitations: information exchange is "bottlenecked via the Planner", and early auto-prompter errors bias the planner.
  
  — [arXiv 2502.10931](https://arxiv.org/html/2502.10931)
- **Plan-and-Act** (SqueezeAILab), arXiv 2503.09572, ICML 2025, v3 22 Apr 2025. Planner and executor are **both LLaMA-3.3-70B-Instruct**, separately fine-tuned, on **WebArena-Lite** (165 tasks).
  - Ablation (extracted; verify column semantics):
    - Base executor with no planner: **9.85%**
    - Executor fine-tuned on WebArena-Lite: 36.36%
    - Adding a planner and progressively better planner training: 43.63%
    - Adding **dynamic replanning after every action**: **53.94%**
    - Adding CoT: **57.58%** (then SOTA)
  - With an *untrained* base executor plus the full dynamic-replanning planner, the system reached 44.24% (extracted).
  - Authors: "the bottleneck may lie more in plan quality than action execution", observed as diminishing returns from more executor data after about 1,113 examples.
  - Other results: WebArena (full) 45.7% (70B) / 48.15% (QwQ-32B); WebVoyager 58.08% (Llama-3.1-8B) / 81.36% (QwQ-32B). With CoT data, an 8B configuration scored 53.33% on WebArena-Lite, "on par with the non-CoT 70B model".
  - Limitations: trajectory generation "depend[s] on having a baseline model that can successfully complete the web tasks", and replanning after every action is slow.
  
  — [arXiv 2503.09572](https://arxiv.org/html/2503.09572); [GitHub](https://github.com/SqueezeAILab/plan-and-act)
- **α-UMi, "Small LLMs Are Weak Tool Learners: A Multi-LLM Agent"** (Shen et al., Alibaba), arXiv 2401.07324, **Jan 2024 [older evidence]**. Planner, caller and summarizer are **all small** (7B/13B), fine-tuned in two stages. This is not a strong/weak split.
  - ToolBench, 7B: plan accuracy 88.92% vs 81.92% for a single LLM; action exact match 58.94% vs 53.26%; hallucination 0.57% vs 2.32%.
  - Real-time ToolBench (DFSDT): α-UMi-7B pass rate **70.9%** vs ChatGPT 64.8% and ToolLLaMA 60.7% (extracted).
  - The authors list "integrating small LLMs with a powerful closed-source LLM like GPT-4" as *future work*, i.e. untested.
  
  — [arXiv 2401.07324](https://arxiv.org/html/2401.07324)
- **ReWOO** (Xu et al.), arXiv 2305.18323, **May 2023 [older evidence]**. Plans all tool calls up front with no observation loop. "5x token efficiency and 4% accuracy improvement on HotpotQA". It "offloads reasoning ability from 175B GPT3.5 into 7B LLaMA", via fine-tuning on multi-step QA, not agentic coding. — [arXiv 2305.18323](https://arxiv.org/abs/2305.18323)
- **LLMCompiler** (Kim et al.), arXiv 2312.04511, ICML 2024 **[older evidence]**. A function-calling planner plus a parallel executor: up to 3.7× latency speedup, up to 6.7× cost savings and up to about 9% accuracy over ReAct. The abstract does not describe using different-strength models for planner and executor. — [arXiv 2312.04511](https://arxiv.org/abs/2312.04511)
- **PlanAhead, "Does The Way You Plan Matter?"** (Zambrano et al.), arXiv 2605.29927, **28 May 2026**.
  - A static planner–executor setup on the 158 WebArena "Hard" tasks. Every planner×executor combination of GPT-4.1-mini, Qwen-2.5-VL-72B and Gemini 2.5 Flash was run, five runs each.
  - Best pair: GPT-4.1-mini planner + Gemini 2.5 Flash executor, **10.7% achievement rate**. Mixed pairs generally beat homogeneous ones.
  - Qwen-2.5-VL self-paired scored 2.5% AR, rising to 5.7% with Gemini as executor.
  - Static plans beat a dynamic single-agent baseline, which showed "action loops and task-tracking failures".
  - The paper does **not** quantify how much of success comes from the plan versus execution. Limitations: three backends, one benchmark, five runs.
  
  — [arXiv 2605.29927](https://arxiv.org/html/2605.29927v1)
- **Strong-to-weak plan injection, CUDA kernel generation** (Chong, Wu, Zhang, Qu, Tsinghua), arXiv 2605.26720, **26 May 2026**, ICML 2026 (PMLR 306).
  - Weaker models: DeepSeek-V3.2 (non-thinking) and Qwen3-Coder-30B-A3B. Stronger: DeepSeek-R1-0528 and Qwen3-235B-A22B.
  - "strong-to-weak plan injection consistently improves execution success for weak models". Gains are larger within one model family (R1 → V3.2, Qwen3-235B → Qwen3-Coder-30B). Plans "partially transfer".
  - "Planning without feedback degrades performance, while feedback-aligned planning yields stable … improvements".
  - Numbers appear only in figures (Fig. 7).
  
  — [arXiv 2605.26720](https://arxiv.org/pdf/2605.26720)
- **"Agentic Systems as Boosting Weak Reasoning Models"** (Sunkaraneni, Beneventano, Neumarker, Poggio, Galanti), arXiv 2605.14163, **13 May 2026**.
  - Single GPT-5.4 nano: **67.0%** on SWE-bench Verified.
  - Eight proposals plus critic-comparator selection: **76.4%**, matching Gemini 3 Pro / Claude Opus 4.5 Thinking solo.
  - Oracle best-of-8: **79.0%**.
  - Ceiling: "oracle best-of-k converges only to the mass of task slices on which the proposal system assigns nonzero useful probability", and "remaining failures are mostly proposal-coverage failures, indicating shared blind spots that stronger selection alone cannot close."
  
  — [arXiv 2605.14163](https://arxiv.org/abs/2605.14163)
- **Reverse-direction "advisor" work (the weak model advises the strong one):** "Weak-for-Strong" (arXiv 2504.04785, 2025) trains a 7B meta-agent to design workflows for a fixed stronger executor such as GPT-4o. "Tiny Advisors for Runtime Intervention" (arXiv 2608.21027, 2026) uses a tiny model that "neither solves the task nor generates task-specific corrections". Both keep the **strong** model as executor. I did not extract their numbers. — [arXiv 2504.04785](https://arxiv.org/pdf/2504.04785); [arXiv 2608.21027](https://arxiv.org/html/2608.21027)

### Inferences
- The evidence splits by who the executor is:
  - When planner and executor are **the same capable model**, better planning can dominate. Plan-and-Act: 70B/70B, fine-tuned, short web actions.
  - When a strong planner is given a **weaker multi-turn tool-calling executor**, performance falls below the strong-homogeneous agent. D-CIPHER: −6 pp absolute (−32% relative) for Sonnet → Haiku, −4 pp (−38% relative) for GPT-4o → 4o-mini.
  
  Duet matches the second case, and its executor is smaller than Haiku or 4o-mini.
- Plan-and-Act's "bottleneck is plan quality" claim does not carry over to duet. That executor was a fine-tuned 70B model on a benchmark whose actions are single clicks and typing, and the plan was **re-generated after every action** from the latest observation. This per-step replanning added about 10 pp.
- The boosting paper gives a mechanism relevant to "frontier reviews while local executes": review or selection cannot add solutions the executor never produces. Here the proposer was GPT-5.4 nano, a far stronger executor than a typical local model, and selection still stopped at the proposer's coverage ceiling (79% oracle).
- The CUDA result (plans help weak models; "planning without feedback degrades performance") suggests plans need to be grounded in execution feedback. Duet's frontier model plans *up front* and reviews afterwards, so it sees execution feedback only through the local model's trajectory. The paper does not state this directly.

### Gaps
- I found no published study with a **frontier planner/reviewer and a small (≤32B) local executor running every tool-calling turn on SWE-bench or a code-creation benchmark**. This is the exact duet configuration, and it appears unstudied.
- D-CIPHER did not report homogeneous Haiku-only or 4o-mini-only scores in what I extracted. So I cannot say whether the mixed pairs fell below *both* single-model agents, which is the duet pattern.
- Plan-and-Solve (arXiv 2305.04091) is a single-model prompting method with no strong/weak split, so it was not pursued.
- Numbers for the advisor papers were not collected.

---

## 5. How much of an agent's quality comes from per-turn execution decisions vs the up-front plan?

### Takeaway
No source I found directly partitions agent quality between the up-front plan and per-turn execution decisions. Indirect evidence says execution-time information and decisions carry much of the weight:
- **Per-step replanning** added about 10 pp in Plan-and-Act.
- Oracle **execution-derived** signals dominate SWE-bench outcomes in ORACLE-SWE: a reproduction test adds +36 pp versus +13 pp for edit location.
- Agent routers find episodes mix high-stakes and routine steps, and the high-stakes ones are hard to identify before execution.

### Cited Findings
- **ORACLE-SWE** (Li, Jin, Zhu, et al.), arXiv 2604.07789, 9 Apr 2026 (revised 28 May 2026). Uses SWE-agent (bash plus a string-replace editor) and gives the agent perfect ("oracle") versions of five signals.
  - **SWE-bench Verified, GPT-4o** (baseline 39.4%):
    - Reproduction test: **75.5% (+36.1 pp)**
    - Edit location: 52.6% (+13.2)
    - Execution context: 48.8% (+9.4)
    - API usage: 45.4% (+6.0)
    - Regression test: 43.0% (+3.6)
  - **SWE-bench-Live, GPT-5** (baseline 27.2%): reproduction test 65.3%; execution context 51.7%; API usage 50.2%; edit location 43.8%.
  - **SWE-bench-Pro/Python, GPT-5** (baseline 25.3%): reproduction test 60.5%.
  - "With all five factors, all four model-bench pairings reach at least 97% success."
  - Main conclusion: "High-quality reproduction tests that capture corner cases are the most influential signal."
  
  — [arXiv 2604.07789](https://arxiv.org/html/2604.07789)
- **Plan-and-Act:** on WebArena-Lite, adding dynamic replanning after every executor action raised success from 43.63% to 53.94% (extracted). The plan is regenerated from each new observation. — [arXiv 2503.09572](https://arxiv.org/html/2503.09572)
- **MTRouter:** episodes contain steps "from high-stakes strategic planning to routine tool invocations". Routing per turn beat the single-model GPT-5 baseline on both quality and cost. Emergent specialization appeared by turn type: GPT-5 lift 1.51× on Python-tool turns, DeepSeek 1.66× on search, Kimi 1.98× on browsing. — [arXiv 2604.23530](https://arxiv.org/html/2604.23530)
- **SWE-Router:** whether an issue is "a localized typo or a multi-module refactor" is "often not identifiable from q alone". The first few execution turns carry the signal needed to decide how much capability the task needs. — [arXiv 2607.00053](https://arxiv.org/html/2607.00053)
- **PlanAhead:** a static up-front plan beat a dynamic single-agent that re-planned inside its loop, because the dynamic agent looped and lost track of the task. The best static result was still only 10.7% achievement rate on WebArena Hard. — [arXiv 2605.29927](https://arxiv.org/html/2605.29927v1)
- **CUDA plan-transfer paper:** "Planning without feedback degrades performance, while feedback-aligned planning yields stable generation-level improvements." — [arXiv 2605.26720](https://arxiv.org/pdf/2605.26720)
- **Minions:** the gain from Minion to MinionS came from restructuring *what the local model does per call* (single-step, short context), not from a better plan text. Splitting multi-part instructions alone was worth 56 points. — [arXiv 2502.15964](https://arxiv.org/html/2502.15964)

### Inferences
- In ORACLE-SWE, the signals a planner could plausibly supply up front are edit location and API usage, worth about +6 to +13 pp for GPT-4o on SWE-bench Verified. The largest signals (reproduction tests, runtime execution context) are produced and used **during execution**. In duet, those are exactly the turns handled by the local model. This is my inference: ORACLE-SWE gives these as perfect oracles, not as outputs of a planner or executor.
- The evidence suggests that much of an agent's quality is decided in execution turns: reading test output, choosing the next file, writing the patch. An up-front plan from a stronger model does not substitute for those decisions unless it is re-derived from observations every step, as in Plan-and-Act. That lowers the value of a frontier planner whose influence arrives only at plan and review time.
- For a *creation* task, where no reproduction test exists up front and the artifact is built turn by turn, the share of quality decided in execution turns is likely higher still. No source measured creation tasks specifically.

### Gaps
- I found **no study that directly decomposes agent success into plan-quality and per-turn-execution contributions** (for example, crossing plan source with executor at fixed tasks and reporting a variance split). PlanAhead's crossed design comes closest but does not report such an attribution.
- I found no evidence specific to **greenfield creation or code-generation** tasks. All agentic evidence above is issue repair (SWE-bench), web navigation (WebArena), CTF, ScienceWorld, HLE or CUDA kernel optimization.
- I found no published case of a strong+weak split scoring below **both** single-model baselines (the duet pattern). Aider shows pairs below the stronger solo model. D-CIPHER shows mixed pairs below strong-homogeneous, but the weak-homogeneous baseline was not extracted. SWE-Router's warning that conditioning on the weak model's reasoning biases the strong model toward the weak model's mistakes is the closest mechanism I found for why a mixed system could fall below both.
