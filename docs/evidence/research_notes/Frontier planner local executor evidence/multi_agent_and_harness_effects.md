# Multi-agent failure modes and harness effects on the same model (coding-agent focus)

Scope note: every source below was retrieved on 2026-09-22. Where a page is summarized, figures were checked against the primary text (arXiv HTML, vendor engineering blog) whenever possible. One automated summary of MAST v2 returned intervention numbers that do not match the paper text; those numbers were discarded and the verified table values are used. Sources older than 2025 are marked **[OLDER EVIDENCE]**.

---

## 1. "Why Do Multi-Agent LLM Systems Fail?" (Cemri, Pan, Yang et al., UC Berkeley; MAST taxonomy)

### Takeaway
MAST studies 7 open-source multi-agent systems (MAS), mostly coding-oriented, over 1,642 traces. It finds failure rates of 41%–86.7%. Most failures come from system design and inter-agent coordination, not from the base model. The best prompt or topology fix with the same model added at most +15.6 points (ChatDev ProgramDev 25.0% → 40.6%), and the authors say these tactical fixes are "insufficient".

### Cited Findings
- **Versions and dates:** v1 was submitted 17 Mar 2025, and v3 (the current version) is dated 26 Oct 2025. v1 was titled around "MASFT". It covered "five popular MAS frameworks across over 150 tasks" with six expert annotators. v3 grew to "MAST-Data", with 1,642 annotated traces from 7 frameworks. — [arXiv abs 2503.13657](https://arxiv.org/abs/2503.13657); [v1 HTML](https://arxiv.org/html/2503.13657v1); [v3 HTML](https://arxiv.org/html/2503.13657v3)
- **Headline claim (v3 abstract):** "Despite enthusiasm for Multi-Agent LLM Systems (MAS), their performance gains on popular benchmarks are often minimal." The introduction says gains "often remain minimal compared to single-agent frameworks or simple baselines like best-of-N sampling", and reports a "41% to 86.7% failure rate on 7 state-of-the-art (SOTA) open-source MAS". — [v3 HTML](https://arxiv.org/html/2503.13657v3)
- **v1 framing:** "the correctness of the state-of-the-art (SOTA) open-source MAS, ChatDev, can be as low as 25%". — [v1 HTML](https://arxiv.org/html/2503.13657v1)
- **Systems, benchmarks and models (v3 Table 1):**
  - ChatDev (ProgramDev, GPT-4o)
  - MetaGPT (ProgramDev, GPT-4o)
  - HyperAgent (SWE-Bench Lite, Claude-3.7-Sonnet)
  - AppWorld (Test-C, GPT-4o)
  - AG2/MathChat (GSM-Plus, GPT-4)
  - Magentic-One (GAIA, GPT-4o)
  - OpenManus (ProgramDev, GPT-4o)
  - LLM-annotated sets of 100 traces each: ChatDev and MetaGPT on ProgramDev-v2 with GPT-4o, Claude-3.7-Sonnet, **Qwen2.5-Coder-32B-Instruct** and **CodeLlama-7b-Instruct**
  - AG2 on OlympiadBench, GSM-Plus and MMLU

  Architecture labels: MetaGPT is an "Assembly Line" simulating software-company SOPs. ChatDev is a "Hierarchical Workflow" simulating design/code/QA phases. HyperAgent is a "Hierarchical Workflow" with a central Planner agent. — [v3 HTML](https://arxiv.org/html/2503.13657v3)
- **Annotation reliability:** κ = 0.88 among three expert annotators. The LLM annotator scored κ = 0.77 against the experts, and κ = 0.79 on unseen systems. — [v3 HTML](https://arxiv.org/html/2503.13657v3) (via WebFetch summary; κ = 0.88 also confirmed in the v1 text)
- **The 14 failure modes with prevalence (v3 text, share of all observed failures):**
  - **FC1 System Design Issues** (named "Specification and System Design Failures" in v1):
    - FM-1.1 Disobey task specification: 11.8%
    - FM-1.2 Disobey role specification: 1.5%
    - **FM-1.3 Step repetition: 15.7%** (the single largest mode)
    - FM-1.4 Loss of conversation history: 2.80%
    - FM-1.5 Unaware of termination conditions: 12.4%
  - **FC2 Inter-Agent Misalignment:**
    - FM-2.1 Conversation reset: 2.20%
    - FM-2.2 Fail to ask for clarification: 6.80%
    - FM-2.3 Task derailment: 7.40%
    - FM-2.4 Information withholding: 0.85%
    - FM-2.5 Ignored other agent's input: 1.90%
    - FM-2.6 Reasoning-action mismatch: 13.2%
  - **FC3 Task Verification** (named "Task Verification and Termination" in v1):
    - FM-3.1 Premature termination: 6.20%
    - FM-3.2 No or incomplete verification: 8.20%
    - FM-3.3 Incorrect verification: 9.10%

  — [v3 HTML](https://arxiv.org/html/2503.13657v3)
- **Category totals:** the category percentages appear only in the Figure 1 image. My own sum of the per-mode values above gives FC1 ≈ 44.2%, FC2 ≈ 32.4% and FC3 ≈ 23.5%. An automated summary of the paper reported different totals: FC1 41.8% / 41.77%, FC2 32.35% / 36.94%, FC3 23.5% / 21.30%, depending on the version fetched. I could not confirm these in the text, so treat category totals as approximate. — [v3 HTML](https://arxiv.org/html/2503.13657v3)
- **FM definitions relevant to handoffs:**
  - FM-2.1 Conversation reset: "Unexpected or unwarranted restarting of a dialogue, potentially losing context and progress made in the interaction."
  - FM-2.4 Information withholding: "Failure to share or communicate important data or insights that an agent possess and could impact decision-making of other agents."

  — [v3 HTML](https://arxiv.org/html/2503.13657v3)
- **Insight 1 (authors):** "MAS failure is not merely a function of challenges in the underlying model; a well-designed MAS can result in performance gain when using the same underlying model." FC1 failures "often reflect flaws in pre-execution design choices regarding system architecture, prompt instructions, or state management." — [v3 HTML](https://arxiv.org/html/2503.13657v3)
- **Insight 2 (authors):** "Solutions focused on context or communication protocols are often insufficient for FC2 failures, which demand deeper 'social reasoning' abilities from agents." — [v3 HTML](https://arxiv.org/html/2503.13657v3)
- **Insight 3 (authors):** "Multi-Level Verification is Needed. Current verifier implementations are often insufficient; sole reliance on final-stage, low-level checks is inadequate." Systems with explicit verifiers (MetaGPT, ChatDev) show fewer total failures, "However, the presence of a verifier is not a silver bullet." Example: a ChatDev chess program "passes superficial checks (e.g., code compilation) but contains runtime bugs." — [v3 HTML](https://arxiv.org/html/2503.13657v3)
- **Intervention results (identical in v1 and v3; v3 Table 5):** all runs use the same model.

  | System | Benchmark / model | Baseline | Improved prompt | New topology |
  |---|---|---|---|---|
  | ChatDev | ProgramDev (32 tasks) | 25.0% | 34.4% | 40.6% |
  | ChatDev | HumanEval | 89.6% | 90.3% | 91.5% |
  | AG2/MathChat | GSM-Plus, GPT-4 | 84.75 ± 1.94 | 89.75 ± 1.44 | 85.50 ± 1.18 |
  | AG2/MathChat | GSM-Plus, GPT-4o | 84.25 ± 1.86 | 89.00 ± 1.38 | 88.83 ± 1.51 |

  For AG2 with GPT-4, the new topology's gain was not significant (Wilcoxon p = 0.4). With GPT-4o, p = 0.03 for both interventions. The ChatDev ProgramDev changes decompose as: the CEO-final-say / role workflow fix = "+9.4%", and adding "a high-level task objective verification step" = "+15.6%". — [v1 HTML](https://arxiv.org/html/2503.13657v1); [v3 HTML](https://arxiv.org/html/2503.13657v3)
- **Authors' verdict on the fixes:**
  - v1: "even though our interventions are successful in improving the performance of the framework in different tasks, they do not constitute substantial improvements."
  - v1 abstract: the improved performance "remains insufficiently low for real-world deployment."
  - v3: "Although first step interventions lead to performance gains, not all failure modes are resolved, and task completion rates still remain low, indicating that more substantial improvements are needed." Also: "With the same underlying model, we achieve max improvements of 15.6%."

  — [v1 HTML](https://arxiv.org/html/2503.13657v1); [v3 HTML](https://arxiv.org/html/2503.13657v3)
- **Organizational framing:** "we conjecture that improvements in the base model capabilities will be insufficient to address the full MAST. Instead, we argue that good MAS design requires organizational understanding – even organizations of sophisticated individuals can fail catastrophically if the organization structure is flawed." This cites Perrow (1984) and high-reliability-organization research. — [v3 HTML](https://arxiv.org/html/2503.13657v3)
- **Open-source models in MAS (v3 Appendix I, Table 6; failure-mode occurrences in 100-trace sets on ProgramDev-v2):**

  | Failure mode | Qwen2.5-Coder-32B, ChatDev | Qwen2.5-Coder-32B, MetaGPT | CodeLlama-7B, ChatDev | CodeLlama-7B, MetaGPT |
  |---|---|---|---|---|
  | FM-1.3 step repetition | 96 | 35 | 97 | 99 |
  | FM-1.5 unaware of termination | 94 | 3 | 97 | 76 |
  | FM-3.2 incomplete verification | 16 | 51 | 67 | 55 |
  | FM-3.3 incorrect verification | 12 | 32 | 69 | 56 |

  The authors state that both open-source models "show a higher frequency of failures compared to the leading closed-source models analyzed in our paper (GPT-4o and Claude-3)." — [v3 HTML](https://arxiv.org/html/2503.13657v3)
- **Architecture changes the failure profile with the same model (Appendix F):** with GPT-4o, MetaGPT has fewer FC1/FC2 failures than ChatDev but "considerably higher" FC3 failures. The authors attribute this to MetaGPT relying on SOPs, whereas ChatDev emphasizes testing and review phases. — [v3 HTML](https://arxiv.org/html/2503.13657v3)
- **MAST reused for single agents:** the Terminal-Bench 2.0 authors (Jan 2026) built their single-agent failure taxonomy from MAST. They reduced it to Execution, Coherence and Verification, because several MAST subcategories "were indistinguishable or irrelevant" for a single agent. — [Terminal-Bench paper, arXiv 2601.11868](https://arxiv.org/html/2601.11868)

### Inferences
- The most common MAST modes map directly onto a phase-machine design: step repetition (15.7%), unawareness of termination conditions (12.4%) and reasoning-action mismatch (13.2%). The small modes, conversation reset (2.2%) and loss of history (2.8%), are the ones that hand-built "context rebuilt at each pass" designs create on purpose. The observed rate of these modes is low in MAST partly because most studied MAS keep a shared chat log, which duet does not.
- MAST's own ceiling is revealing. The best same-model design fix gained about +15.6 points on a coding benchmark, from a 25% baseline. Organizational structure therefore moves results a lot, in both directions. A badly structured MAS can plausibly lose that much or more relative to a simpler design.
- The local-model table suggests that a ~30B open coder inside a multi-phase MAS shows very high step-repetition and termination-awareness failure counts: 94–96 of 100 traces in ChatDev. That is the same failure signature one would predict for a local executor that is re-briefed at each pass.

### Gaps
- MAST does not compare each MAS against a matched single-agent loop on the same tasks. The "minimal gains vs single-agent" claim cites prior work (Xia et al. 2024; Kapoor et al. 2024) rather than a new measurement.
- The category-level percentages could not be verified from the text (they appear only in a figure). The per-mode numbers are verified.

---

## 2. Cognition: "Don't Build Multi-Agents" (Walden Yan, 12 Jun 2025) and later revisions

### Takeaway
Cognition's 2025 post argues that context must be fully shared (full traces, not just messages) and that "actions carry implicit decisions". It concludes that parallel or multi-writer agents are "fragile" and that a single-threaded linear agent should be the default. Its April 2026 follow-up sharpens this rather than retracting it. Multi-agent setups that work keep writes single-threaded and use extra agents only for "intelligence", such as a clean-context reviewer or a "smart friend". A weaker primary model escalating to a stronger one is explicitly called unsolved.

### Cited Findings
- **Date, author and principles:** Walden Yan, 06.12.25.
  - "Principle 1: Share context, and share full agent traces, not just individual messages."
  - "Principle 2: Actions carry implicit decisions, and conflicting decisions carry bad results."
  - "Principles 1 & 2 are so critical, and so rarely worth violating, that you should by default rule out any agent architectures that don't abide by them."

  — [Cognition, Don't Build Multi-Agents](https://cognition.com/blog/dont-build-multi-agents)
- **Handoff argument:** copying the original task to subagents is not enough, because "in a real production system, the conversation is most likely multi-turn, the agent probably had to make some tool calls to decide how to break down the task, and any number of details could have consequences on the interpretation of the task." Flappy Bird example: one subagent builds a Super-Mario-style background, and the other builds a mismatched bird. Even with shared task context, "Subagent 1 and subagent 2 cannot not see what the other was doing and so their work ends up being inconsistent." — [Cognition 2025](https://cognition.com/blog/dont-build-multi-agents)
- **Recommended architecture:** "The simplest way to follow the principles is to just use a single-threaded linear agent … the context is continuous." For very long tasks, add a model "whose key purpose is to compress a history of actions & conversation into key details, events, and decisions. This is hard to get right." Cognition says it has fine-tuned a smaller model for this. — [Cognition 2025](https://cognition.com/blog/dont-build-multi-agents)
- **Claude Code subagents (as of June 2025):** Claude Code "never does work in parallel with the subtask agent, and the subtask agent is usually only tasked with answering a question, not writing any code. Why? The subtask agent lacks context from the main agent that would otherwise be needed to do anything beyond answering a well-defined question." — [Cognition 2025](https://cognition.com/blog/dont-build-multi-agents)
- **Large-model-plans / small-model-applies (the 2024 "edit apply model" pattern):** "the small model would misinterpret the instructions of the large model and make an incorrect edit due to the most slight ambiguities in the instructions. Today, the edit decision-making and applying are more often done by a single model in one action." — [Cognition 2025](https://cognition.com/blog/dont-build-multi-agents)
- **Conclusion:** "in 2025, running multiple agents in collaboration only results in fragile systems. The decision-making ends up being too dispersed and context isn't able to be shared thoroughly enough between the agents." — [Cognition 2025](https://cognition.com/blog/dont-build-multi-agents)
- **Follow-up, "Multi-Agents: What's Actually Working" (Walden Yan, 22 Apr 2026):** "multi-agent systems work best today when writes stay single-threaded and the additional agents contribute intelligence rather than actions." Three patterns work: Code-Review-Loop, Smart Friend, and map-reduce-and-manage. "Unstructured swarm … is mostly a distraction." — [Cognition, Multi-Agents: What's Actually Working](https://cognition.com/blog/multi-agents-working)
- **Code-Review-Loop:**
  - The reviewer works best with "completely clean context", because of context rot. It "gets to skip this extraneous context, only look at the diff, and re-discover any context it needs."
  - Using the same model in both roles is fine: it "does not quite make them self-biased/correlated in the same way you might imagine."
  - Results: "Devin Review catches an average of 2 bugs per PR, of which roughly 58% are severe."
  - Cost caveat: "Often the system will loop through multiple code-review cycles, finding new bugs each time (which isn't always great since it can take a while)."

  — [Cognition 2026](https://cognition.com/blog/multi-agents-working)
- **Smart Friend (a weaker primary consults a stronger model):**
  - "The core trickiness … 'how does a dumber model know it's at its limits?'"
  - "SWE 1.5 was not good enough at being the primary model for this setup to really work. The gap between it and Sonnet 4.5 was too wide in exactly the places that mattered … knowing when to escalate, knowing what to ask."
  - "Getting it to work with an asymmetrically weaker primary, which is the version that leads to the biggest unlocks, is still an open problem, and we think it's a training one."

  — [Cognition 2026](https://cognition.com/blog/multi-agents-working)
- **Map-reduce-and-manage caveat:** "Managers trained on small-scoped delegation default to being overly prescriptive, which backfires when the manager lacks deep codebase context." Open problems "are all communication problems … How do you transfer context between agents without drowning the receiver?" — [Cognition 2026](https://cognition.com/blog/multi-agents-working)

### Inferences
- Duet's design (a frontier planner writing a brief, and a local executor rebuilt from that brief each pass) is structurally the "large model explains, small model applies" pattern that Cognition says was abandoned. It also violates Principle 1: the executor receives a brief, not the full trace.
- The 2026 follow-up points to a specific failing combination: an asymmetric capability gap, cross-agent context transfer, and a manager (planner) without deep codebase context who is "overly prescriptive". Cognition calls this combination an open, training-level problem. Duet's worse-than-plain-loop result fits this rather than contradicting it.
- Cognition's endorsed reviewer pattern differs from duet's in two ways that may matter. The Cognition reviewer is advisory to a single writer that keeps its own context. Its review is not a gating phase that tears down and rebuilds the writer's context.

### Gaps
- Cognition gives no controlled A/B numbers (single-agent vs multi-agent success rates) in either post. The evidence is practitioner experience plus Devin Review bug counts.

---

## 3. Anthropic: the multi-agent research system (13 Jun 2025), "Building effective agents" (19 Dec 2024), and later harness posts

### Takeaway
Anthropic's multi-agent research system beat a single agent by 90.2% on a breadth-first research eval. It did so mainly by spending more tokens: token usage alone explained 80% of the variance, and multi-agent runs used about 15× the tokens of chat. Anthropic explicitly says most coding tasks are a poor fit. Its guidance since Dec 2024 is "the simplest solution possible". Its 2025–2026 harness posts show that planner and evaluator agents can help frontier models at a large cost, and that such scaffolding should be removed as models improve.

### Cited Findings
- **Multi-agent research system (published 13 Jun 2025):** "a multi-agent system with Claude Opus 4 as the lead agent and Claude Sonnet 4 subagents outperformed single-agent Claude Opus 4 by 90.2% on our internal research eval." — [Anthropic, How we built our multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
- **Variance explained:** "Multi-agent systems work mainly because they help spend enough tokens to solve the problem. In our analysis, three factors explained 95% of the performance variance in the BrowseComp evaluation … token usage by itself explains 80% of the variance, with the number of tool calls and the model choice as the two other explanatory factors." Also: "upgrading to Claude Sonnet 4 is a larger performance gain than doubling the token budget on Claude Sonnet 3.7." — [Anthropic multi-agent research](https://www.anthropic.com/engineering/multi-agent-research-system)
- **Token multipliers:** "agents typically use about 4× more tokens than chat interactions, and multi-agent systems use about 15× more tokens than chats. For economic viability, multi-agent systems require tasks where the value of the task is high enough to pay for the increased performance." — [Anthropic multi-agent research](https://www.anthropic.com/engineering/multi-agent-research-system)
- **Poor-fit domains (verbatim):** "some domains that require all agents to share the same context or involve many dependencies between agents are not a good fit for multi-agent systems today. For instance, most coding tasks involve fewer truly parallelizable tasks than research, and LLM agents are not yet great at coordinating and delegating to other agents in real time." — [Anthropic multi-agent research](https://www.anthropic.com/engineering/multi-agent-research-system)
- **Early coordination failures observed:** "spawning 50 subagents for simple queries, scouring the web endlessly for nonexistent sources", and subagents that "performed the exact same searches as other agents". Also: "minor system failures can be catastrophic for agents", and the synchronous design means "the entire system can be blocked while waiting for a single subagent." — [Anthropic multi-agent research](https://www.anthropic.com/engineering/multi-agent-research-system)
- **"Building effective agents" (Erik S. and Barry Zhang, 19 Dec 2024) [OLDER EVIDENCE]:**
  - "we recommend finding the simplest solution possible, and only increasing complexity when needed. This might mean not building agentic systems at all. Agentic systems often trade latency and cost for better task performance."
  - "The autonomous nature of agents means higher costs, and the potential for compounding errors."
  - The three principles: simplicity, transparency, and a carefully crafted agent-computer interface (ACI).
  - "While building our agent for SWE-bench, we actually spent more time optimizing our tools than the overall prompt." Example: switching to absolute filepaths, after which "the model used this method flawlessly."
  - Evaluator-optimizer loops fit "when we have clear evaluation criteria, and when iterative refinement provides measurable value."

  — [Anthropic, Building effective agents](https://www.anthropic.com/engineering/building-effective-agents)
- **"Effective context engineering for AI agents" (29 Sep 2025):**
  - Context rot: "as the number of tokens in the context window increases, the model's ability to accurately recall information from that context decreases."
  - Compaction risk: "overly aggressive compaction can result in the loss of subtle but critical context whose importance only becomes apparent later."
  - Subagents return "a condensed, distilled summary of its work (often 1,000–2,000 tokens)" after exploring with "tens of thousands of tokens or more".
  - Tool sets: avoid bloated tool sets where "a human engineer can't definitively say which tool should be used."

  — [Anthropic, Effective context engineering](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)
- **"Effective harnesses for long-running agents" (26 Nov 2025):**
  - Core problem: "each new session begins with no memory of what came before."
  - Observed failures: trying to one-shot the whole app; later sessions declaring the project finished after seeing existing progress; leaving broken state for the next session; marking features done without end-to-end tests.
  - Fix: an initializer agent, plus a coding agent that works on one feature per session, with a JSON feature list, a progress file, git commits and init.sh. "Compaction isn't sufficient."
  - Open question: whether "a single, general-purpose coding agent performs best across contexts, or if better performance can be achieved through a multi-agent architecture."

  — [Anthropic, Effective harnesses for long-running agents](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)
- **"Harness design for long-running application development" (Prithvi Rajasekaran, 24 Mar 2026), a planner + generator + evaluator harness:**
  - Solo agent: 20 min and $9, producing a broken game ("entities appeared on screen but nothing responded to input").
  - Full harness: 6 hr and $200, producing a working app.
  - Self-evaluation bias: agents "confidently prais[e] the work—even when … the quality is obviously mediocre."
  - The evaluator initially would "identify legitimate issues, then talk itself into deciding they weren't a big deal and approve the work anyway", and needed multiple prompt iterations.
  - "Every component in a harness encodes an assumption about what the model can't do on its own, and those assumptions are worth stress testing, both because they may be incorrect, and because they can quickly go stale as models improve."
  - On Opus 4.6 they removed sprint decomposition and per-sprint grading. The simplified run took 3 hr 50 min and cost $124.70; the builder alone took 2 hr 7 min and cost $71.08.
  - The evaluator is worth it only when "the task sits beyond what the current model does reliably solo". Otherwise it becomes "unnecessary overhead".
  - "Context anxiety": models wrap up prematurely near a perceived context limit. "Compaction preserves continuity, it doesn't give the agent a clean slate … A reset provides a clean slate." For Sonnet 4.5, "context resets became essential."

  — [Anthropic, Harness design for long-running apps](https://www.anthropic.com/engineering/harness-design-long-running-apps)
- **Claude 3.7 Sonnet on SWE-bench Verified (24 Feb 2025):**
  - The standard scaffold (bash tool + string-replace editor + "planning tool", single session) scored 63.7% on 489 tasks.
  - The "high-compute" scaffold scored 70.3%. It used parallel attempts, rejection of patches that break visible regression tests, and a scoring model to pick the best attempt.

  The gain comes from parallel sampling plus test-based selection, not from serial multi-agent handoffs. — [Anthropic, Claude 3.7 Sonnet announcement](https://www.anthropic.com/news/claude-3-7-sonnet)

### Inferences
- Anthropic's positive multi-agent result is for breadth-first, parallelizable research, and its explanation is mostly "more tokens". Duet runs on a local model, where compute is the scarce resource, and on coding tasks that Anthropic flags as a poor fit. That is the opposite regime.
- Anthropic's own planner/evaluator harness helped a frontier model only at 22× the cost and 18× the time of the solo run. Anthropic then stripped it back when the model improved. Its rule, "every component encodes an assumption about what the model can't do", is a direct test for duet's phase machine, pass limits and host refusals. Each of these encodes an assumption, and the plain-loop result suggests several are wrong for this model and task mix.
- Where Anthropic's harnesses get benefit from resets and handoffs, they pair them with durable, repo-resident state: a feature list, a progress file, git history. They do not rely on a natural-language brief alone.

### Gaps
- Anthropic does not publish a matched single-agent vs multi-agent result on a coding benchmark. The "coding is a poor fit" claim is qualitative.
- The 24 Mar 2026 harness post reports single demo runs (a game and a DAW), not a benchmark with variance.

---

## 4. Harness effects on the same model: SWE-agent ACI, Agentless, mini-SWE-agent, Terminal-Bench, and 2025–2026 evidence

### Takeaway
With the model held fixed, the harness routinely moves coding-benchmark scores by 5–17 points, and by far more when the edit interface is broken (Grok Code Fast 1 went from 6.7% to 68.3% after an edit-format change). Minimal harnesses are consistently competitive or better. On Terminal-Bench 2.0, the one-tool Terminus 2 beat Claude Code and OpenHands for Claude Opus 4.5 (57.8% vs 52.1% / 51.9%) while using about 66× fewer input tokens than Claude Code. Harness sensitivity is largest for weaker models.

### Cited Findings
- **SWE-agent (Yang, Jimenez et al., Princeton; arXiv 6 May 2024, v3 11 Nov 2024; NeurIPS 2024) [OLDER EVIDENCE]:**
  - SWE-bench Lite, all runs with GPT-4 Turbo:
    - RAG: 2.67%
    - Shell-only agent: 11.00% (7.33% without demonstration)
    - SWE-agent with its ACI: 18.00%
  - Full SWE-bench: SWE-agent 12.47% vs RAG 1.31%. With Claude 3 Opus: 10.46% on full SWE-bench and 13.00% on Lite.
  - The authors: SWE-agent "solves 10.7 percentage points more instances than the baseline agent, which uses only the default Linux shell."
  - Cost: SWE-agent is "8–13x more costly" than RAG, for a 6.7-fold gain.

  — [SWE-agent, arXiv 2405.15793](https://arxiv.org/abs/2405.15793) ([HTML](https://arxiv.org/html/2405.15793))
- **SWE-agent ACI ablations (SWE-bench Lite, GPT-4 Turbo, full configuration = 18.0%):**
  - Editor: edit without linting 15.0 (−3.0); no edit command 10.3 (−7.7)
  - Search: iterative search 12.0 (−6.0); no search 15.7 (−2.3)
  - File viewer: 30-line window 14.3 (−3.7); full file 12.7 (−5.3)
  - Context: **full history 15.0 (−3.0)** vs collapsing observations older than the last 5
  - No demonstration: 16.3 (−1.7)

  Principle stated: "Guardrails mitigate error propagation and hasten recovery." — [SWE-agent HTML](https://arxiv.org/html/2405.15793)
- **Agentless (Xia, Deng, Dunn, Zhang, UIUC; arXiv 1 Jul 2024, v2 29 Oct 2024) [OLDER EVIDENCE]:**
  - A fixed three-phase pipeline (localization → repair → patch validation) "without letting the LLM decide future actions or operate with complex tools" scored "32.00%, 96 correct fixes" on SWE-bench Lite at $0.70 per issue with GPT-4o. That was the best of the open-source entries at the time.
  - Comparisons on the same benchmark: SWE-agent + GPT-4o 18.33% at $2.53; SWE-agent + Claude 3.5 Sonnet 23.00% at $1.62; OpenDevin+CodeAct v1.8 + Claude 3.5 Sonnet 26.67% at $1.14; AutoCodeRover + GPT-4 19.00% at $0.45.
  - Authors' critique of agents: "Limited ability to self-reflect … an incorrect step can be easily amplified and negatively affect all future decisions made by the agent."

  — [Agentless, arXiv 2407.01489](https://arxiv.org/abs/2407.01489) ([HTML](https://arxiv.org/html/2407.01489))
- **mini-SWE-agent (SWE-agent team; README accessed 2026-09-22):** about 100 lines of Python. Bash only, no tool-calling interface, and a linear history ("trajectory and LM prompts are identical"). Claims ">74% on SWE-bench Verified". It powers the SWE-bench "bash only" leaderboard for comparing models under a fixed minimal scaffold. — [mini-swe-agent GitHub](https://github.com/SWE-agent/mini-swe-agent)
- **Terminal-Bench 2.0 paper (Merrill et al., Stanford / Laude Institute / Anthropic and others, arXiv 17 Jan 2026):**
  - The authors built Terminus 2 as "a neutral testbed". It "has a single tool, a headless terminal, and completes tasks using only Bash commands."
  - Their rationale: "Many agent scaffolds have been engineered to accommodate the tendencies of certain models, especially when the model and agent are developed by the same organization."
  - The run covered 32,155 trials, with at least 5 per agent-model pair.

  — [Terminal-Bench, arXiv 2601.11868](https://arxiv.org/abs/2601.11868) ([HTML](https://arxiv.org/html/2601.11868))
- **Terminal-Bench 2.0 same-model spreads.** Resolution rate ± 95% CI from Table 2. My own grouping by model; "spread" is best minus worst harness for that model.

  | Model | Results by harness (input tokens in parentheses) | Spread |
  |---|---|---|
  | Claude Opus 4.5 | Terminus 2 57.8% ±2.5 (3.9M); Claude Code 52.1% ±2.5 (**256.9M**); OpenHands 51.9% ±2.9 (151.4M) | 5.9 pts |
  | GPT-5.2 | Codex CLI 62.9% ±3.0 (137.5M); Terminus 2 54.0% ±2.9 (12.4M) | 8.9 pts |
  | GPT-5 | Codex CLI 49.6%; OpenHands 41.5%; Terminus 2 35.2%; Mini-SWE-Agent 33.9% | 15.7 pts |
  | Gemini 2.5 Pro | Terminus 2 32.6%; Mini-SWE-Agent 26.1%; Gemini CLI 19.6%; OpenHands 15.7% | 16.9 pts |
  | Claude Haiku 4.5 | Mini-SWE-Agent 29.8%; Terminus 2 28.3%; Claude Code 27.5%; OpenHands 13.3% (663.1M input tokens) | 16.5 pts |
  | Claude Sonnet 4.5 | Terminus 2 42.8%; Mini-SWE-Agent 42.5%; OpenHands 40.3%; Claude Code 40.1% | 2.7 pts |
  | Claude Opus 4.1 | Terminus 2 38.0%; Mini-SWE 35.1%; OpenHands 34.9%; Claude Code 34.8% | 3.2 pts |
  | GPT-5-Mini | Codex CLI 31.9%; OpenHands 27.7%; Terminus 2 24.0%; Mini-SWE 22.2% | 9.7 pts |
  | Grok 4 | Mini-SWE 29.0%; Terminus 2 23.4%; OpenHands 19.6% | 9.4 pts |
  | Grok Code Fast 1 | Mini-SWE 24.5%; Terminus 2 14.5% | 10.0 pts |
  | GPT-OSS-120B | Terminus 2 18.7%; Mini-SWE 14.2% | 4.5 pts |
  | Qwen 3 Coder 480B | OpenHands 24.3%; Terminus 2 23.9% | 0.4 pts |
  | Kimi K2 Instruct | Terminus 2 27.8%; OpenHands 25.6% | 2.2 pts |

  Figure 1 of the paper reports each model with "the agent scaffold … chosen to maximize performance". — [Terminal-Bench HTML, Appendix A Table 2](https://arxiv.org/html/2601.11868)
- **Terminal-Bench 2.0 on efficiency:** "verbosity and an extensive number of turns do not inherently lead to better performance … the most effective models demonstrate efficiency in both token generation and episode utilization." — [Terminal-Bench HTML, App. G.3](https://arxiv.org/html/2601.11868)
- **Terminal-Bench failure analysis (Terminus 2 scaffold, MAST-derived):** "Execution errors dominate for Opus 4.5 and GPT-5.2, while coherence and verification errors occur at lower rates. Conversely, the open sourced model evaluated (Qwen Coder) displays a more balanced error pattern, with higher errors across all failure modes." Command error rates range from 9.2% (Grok 4) to 26.7% (GPT-OSS-120B). — [Terminal-Bench HTML §4.4–4.5](https://arxiv.org/html/2601.11868)
- **Epoch AI, "Why benchmarking is hard" (Florian Brand and Jean-Stanislas Denain, 23 Dec 2025):** on SWE-bench Verified, "simply switching the scaffold makes up to an 11% difference for GPT-5 and up to a 15% difference for Kimi K2 Thinking." "The choice of scaffold has the single biggest impact on the overall performance." Provider problems also move scores: rate limits, "empty or cut-off responses", and overstated token limits. — [Epoch AI, Why benchmarking is hard](https://epoch.ai/gradient-updates/why-benchmarking-is-hard)
- **LangChain, "Improving Deep Agents with harness engineering" (Vivek Trivedy, 17 Feb 2026):** with GPT-5.2-Codex fixed, harness-only changes moved Terminal-Bench 2.0 from 52.8% to 66.5% (+13.7), "Top 30 to Top 5". The changes were:
  - A self-verification prompt plus a PreCompletionChecklist middleware
  - Local context injection (directory map, tooling, time-budget warnings)
  - Loop-detection middleware for "doom loops"
  - A reasoning "sandwich" (xhigh, then high, then xhigh)

  xhigh reasoning throughout scored only 53.9% because of timeouts; high throughout scored 63.6%. Most common failure: "the agent wrote a solution, re-read its own code, confirmed it looks ok, and stopped." — [LangChain blog](https://www.langchain.com/blog/improving-deep-agents-with-harness-engineering)
- **Can Bölük, "The harness problem" (12 Feb 2026; now hosted at stencil.so):** 16 models, 180 tasks × 3 runs, changing only the edit tool (apply_patch vs str_replace vs "hashline" content-hash anchors).
  - Hashline beat replace for 14 of 16 models, with an average gain of about +15 points.
  - Grok Code Fast 1: 6.7% → 68.3%. GPT-5.1 Codex Mini: 60.0% → 77.5%. Claude Sonnet 4.5: +14.4 points. Claude Haiku 4.5: +13 points.
  - Output tokens fell by up to about 49%. The weakest models gained the most.
  - The author's line: "You're blaming the pilot for the landing gear."

  This is a practitioner post, not peer-reviewed. — [Stencil / Can Bölük, The harness problem](https://stencil.so/blog/the-harness-problem) (original: blog.can.ac/2026/02/12/the-harness-problem)
- **Pi coding agent (Mario Zechner, 30 Nov 2025):** system prompt under 1,000 tokens. Four tools (read, write, edit, bash). No sub-agents ("a black box within a black box"), no plan mode, no to-do tool ("confuses models more than they help"), and no MCP (Playwright MCP costs "21 tools, 13.7k tokens").
  - Pi with Claude Opus 4.5 was submitted to the Terminal-Bench 2.0 leaderboard. Its score is shown only in images, which I could not extract.
  - On Terminus 2: "No fancy tools, no file operations, just raw terminal interaction. And it's holding its own against agents with far more sophisticated tooling."

  — [Mario Zechner, pi-coding-agent](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/)

### Inferences
- Across independent sources, a same-model harness swap typically moves coding scores by about 3–17 points out of 100. The edit-interface failures in the hashline study show that much larger swings, tens of points, happen when the harness blocks the model mechanically.
- Duet's deficit is 40–43/50 vs 18–39/50, i.e. 2–25 of 50 points, or roughly 4–50 points out of 100. That is at or beyond the top of the normal range, and its low end resembles the "mechanical blockage" regime. Host refusals, pass limits and phase gates are candidates for that kind of blockage, analogous to broken edit tools.
- Heavier harnesses did not buy accuracy with more tokens. Claude Code used about 66× the input tokens of Terminus 2 for Opus 4.5, yet scored 5.7 points lower. OpenHands with Haiku 4.5 used 663M input tokens and scored 13.3%, against 27.5–29.8% for lighter harnesses. Added orchestration can spend compute and lose accuracy at the same time.
- Harness sensitivity is largest for weaker or mid-tier models (Haiku 4.5, Gemini 2.5 Pro, GPT-5-Mini, Grok Code Fast 1). A local executor is likely in that band, so duet's harness choices would be amplified rather than dampened.
- Vendor-matched harnesses help some model families: Codex CLI for GPT-5.x adds about 9–16 points. Claude Code did not beat a minimal loop for Claude models on Terminal-Bench 2.0. "Heavier and more engineered" is not reliably better even when the vendor built it.

### Gaps
- The live tbench.ai URL for Terminal-Bench 2.0 now serves a Terminal-Bench 4.0 leaderboard (entries up to Sep 2026). That table has no same-model/different-agent pairs, so I relied on the paper's Table 2 (Jan 2026 snapshot).
- I could not extract Pi's Terminal-Bench number (image only).
- I could not find a controlled study that swaps a single-loop harness for a planner → executor → reviewer phase machine with the same local model on SWE-bench or Terminal-Bench.
- The Google/DeepMind scaling study (section 6) is the closest match, but it uses frontier API models.

---

## 5. Context loss at handoffs and restarts, summarization/compaction loss, and long-context degradation

### Takeaway
Several measurements point the same way:
- Splitting a task's information across turns costs 39% on average, and a final recap recovers only a fraction.
- LLM summaries can hide failure signals and lengthen trajectories. Simple observation masking matches or beats summarization at half the cost.
- Performance degrades with input length even on trivial tasks. Models condition on their own earlier errors.

Rebuilding an executor from a brief each pass exposes it to the first two problems, and a long single context exposes it to the third.

### Cited Findings
- **"LLMs Get Lost In Multi-Turn Conversation" (Laban, Hayashi, Zhou, Neville; Microsoft Research / Salesforce; 9 May 2025):**
  - Across 15 LLMs and 200,000+ simulated conversations, "all the top open- and closed-weight LLMs we test exhibit significantly lower performance in multi-turn conversations than single-turn, with an average drop of 39% across six generation tasks" (Figure 1 caption: −35%).
  - Aptitude fell only 16%, while "unreliability skyrockets with an average increase of 112% (more than doubling)", with "performance degrading 50 percent points on average between the best and worst simulated run for a fixed instruction."
  - "LLMs often make assumptions in early turns and prematurely attempt to generate final solutions, on which they overly rely … when LLMs take a wrong turn in a conversation, they get lost and do not recover."
  - Remedies: "Snowball" (restating all prior shards every turn) "can mitigate the Full-to-Sharded performance deterioration by 15–20%". The authors conclude "relying on an agent-like framework to process information might be limiting."

  — [Laban et al., arXiv 2505.06120](https://arxiv.org/abs/2505.06120) ([HTML](https://arxiv.org/html/2505.06120))
- **"The Complexity Trap" (Lindenbauer et al., JetBrains Research; arXiv 29 Aug 2025, v3 27 Oct 2025; NeurIPS 2025 DL4C workshop):**
  - Setup: SWE-agent on SWE-bench Verified, five model configurations.
  - Main result: "a simple environment Observation Masking strategy halves cost relative to the Raw Agent while matching, and sometimes slightly exceeding, the solve rate of LLM-Summary."
  - Qwen3-Coder 480B: raw agent 53.4%; masking 54.8% at −52.7% cost; LLM summary 53.8% at −50.4% cost.
  - Gemini 2.5 Flash: raw agent 32.8%; masking 35.6%; summary 36.0%.
  - "Trajectory elongation": LLM summaries raised mean turns by about 15% (Gemini 2.5 Flash: 52 turns vs 44 with masking; Qwen3-Coder 480B: +15% vs raw). The authors suggest summaries "mask failure signals that would otherwise prompt earlier termination."
  - A critic-enhanced summary "showed no improvement in solve rate over standard LLM-Summary" and produced "even longer trajectories".

  — [JetBrains, arXiv 2508.21433](https://arxiv.org/abs/2508.21433) ([HTML](https://arxiv.org/html/2508.21433))
- **Chroma, "Context Rot" (Hong, Troynikov, Huber; 14 Jul 2025):**
  - Across 18 LLMs (GPT-4.1, Claude 4, Gemini 2.5, Qwen3 and others), "model performance degrades as input length increases, often in surprising and non-uniform ways."
  - "Even a single distractor reduces performance relative to the baseline (needle only)."
  - On LongMemEval, focused ~300-token inputs scored significantly better than full ~113k-token prompts.
  - Even replicating repeated words degrades with length.

  — [Chroma Context Rot](https://www.trychroma.com/research/context-rot)
- **"Lost in the Middle" (Liu et al., Stanford; arXiv 6 Jul 2023, TACL) [OLDER EVIDENCE]:** "performance is often highest when relevant information occurs at the beginning or end of the input context, and significantly degrades when models must access relevant information in the middle of long contexts, even for explicitly long-context models." — [arXiv 2307.03172](https://arxiv.org/abs/2307.03172)
- **"The Illusion of Diminishing Returns" (Sinha, Arun, Goel, Staab, Geiping; arXiv 11 Sep 2025; ICLR 2026):**
  - "even marginal gains in single-step accuracy can compound into exponential improvements in the length of tasks a model can successfully complete."
  - Self-conditioning: "models become more likely to make mistakes when the context contains their errors from prior turns. Self-conditioning does not reduce by just scaling the model size."
  - "thinking mitigates self-conditioning."

  — [arXiv 2509.09677](https://arxiv.org/abs/2509.09677)
- **Anthropic on compaction and resets:**
  - Aggressive compaction can lose "subtle but critical context whose importance only becomes apparent later." — [Anthropic, Effective context engineering (29 Sep 2025)](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)
  - "Each new session begins with no memory of what came before." Later sessions "declare projects finished after observing existing progress." — [Anthropic, Effective harnesses (26 Nov 2025)](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)
  - Resets beat compaction for Sonnet 4.5's "context anxiety". — [Anthropic, Harness design (24 Mar 2026)](https://www.anthropic.com/engineering/harness-design-long-running-apps)
- **SWE-agent context ablation [OLDER EVIDENCE]:** keeping the full history scored 15.0% vs 18.0% when observations older than the last 5 were collapsed to one line each (GPT-4 Turbo, SWE-bench Lite). Pruning stale observations helped; history as such was not the problem. — [SWE-agent HTML](https://arxiv.org/html/2405.15793)
- **Handoff-specific MAST modes:** FM-1.4 loss of conversation history (2.80%), FM-2.1 conversation reset (2.20%), FM-2.4 information withholding (0.85%). FM-2.2 fail to ask for clarification (6.80%) is also relevant. — [MAST v3](https://arxiv.org/html/2503.13657v3)
- **Cognition 2026 on reviewer context:** clean context helps the reviewer, because "Context Rot is a well-documented phenomenon." — [Cognition 2026](https://cognition.com/blog/multi-agents-working)

### Inferences
- Two different context problems pull in opposite directions, and duet's design leans into the costlier one for execution:
  - Long single contexts degrade (context rot, lost in the middle, self-conditioning).
  - Rebuilding from a brief loses information. Laban's sharding result means an executor receiving a partial or paraphrased spec behaves as if underspecified, and the recap-style fix recovers only about 15–20% of the loss.
  - The evidence favours pruning or masking stale observations inside one continuous loop (SWE-agent's last-5 collapse, JetBrains' masking) over summarize-and-restart handoffs, at least for SWE-bench-style tasks.
- The JetBrains finding that summaries lengthen trajectories by hiding failure signals, with critic-style reflections lengthening them further, is a measured mechanism by which a brief-plus-reviewer loop can add turns without adding solves.
- Clean context is an advantage for a reviewer that only reads a diff (Cognition). It is a liability for an executor that must also rediscover codebase state and the reasons behind earlier decisions (Cognition Principle 2, Anthropic's "no memory" failures).

### Gaps
- I found no study measuring coding-agent success specifically as a function of "context rebuilt from a planner-written brief at each pass" vs continuous context with the same local model.
- Laban et al. study conversational underspecification, not executor briefs. The mapping is my inference.
- The Chroma study is vendor research (Chroma sells retrieval infrastructure), although its methods are published.

---

## 6. Costs of added orchestration steps and verification rounds (tokens, latency, error compounding)

### Takeaway
Controlled evidence from 2025–2026 finds that multi-agent coordination helps decomposable, parallel tasks but hurts sequential ones. On SWE-bench Verified every multi-agent architecture was 2–15% below the single agent, and above a ~45% single-agent baseline coordination has negative returns. Extra agents and verification rounds cost 4–15× the tokens (or 22× the dollars in one Anthropic demo) and add turns. Verification pays only when it adds information: running tests, catching premature stops, or a task beyond the model's solo reach. An LLM reviewer or critic without new signal can be lenient, lengthen trajectories, or loop.

### Cited Findings
- **"Towards a Science of Scaling Agent Systems" (Kim, Gu, Park et al., Google Research / MIT / others; arXiv 9 Dec 2025, v3 8 Apr 2026):**
  - Scope: 260 controlled configurations across 6 benchmarks (BrowseComp-Plus, Finance-Agent, PlanCraft, Workbench, **SWE-bench Verified**, **Terminal-Bench**), three model families (OpenAI, Google, Anthropic), and five architectures (single agent, plus independent, centralized, decentralized and hybrid MAS) "with matched compute".
  - Per-benchmark relative change of MAS vs a single agent:

    | Benchmark | MAS change vs single agent |
    |---|---|
    | Finance Agent | +57% to +80.8% |
    | PlanCraft (sequential) | −39% to −70% |
    | Workbench | −11% to +6% |
    | **SWE-bench Verified** | **"slight degradation across all MAS architectures (from −15% to −2%), consistent with high single-agent baselines (>45%)"** |
    | Terminal-Bench | independent +2%; centralized −19% ("reflecting the low tool count (2 tools) where coordination overhead…") |

  - Three effects:
    - A tool-coordination trade-off: tool-heavy tasks "suffer from multi-agent coordination overhead", because MAS "fragment the per-agent token budget".
    - A capability ceiling (β = −0.236, p = 0.004): "tasks where single-agent performance already exceeds 45% accuracy experience negative returns from additional agents."
    - Error amplification: "Independent systems amplify trace-level errors 17.2× … Centralized coordination … contains this to 4.4× by enforcing validation bottlenecks."
  - Also measured: "Turn count follows power-law scaling with number of agents."
  - Framing: "Single-agent systems maximize context integration by maintaining a unified memory stream in which all reasoning steps share full access to prior history."

  — [arXiv 2512.08296](https://arxiv.org/abs/2512.08296) ([HTML](https://arxiv.org/html/2512.08296))
- **"Do More Agents Help?" (Fu et al.; arXiv 4 Jun 2026):** with the benchmark loader, tool access, answer contract and usage accounting matched, "At most one of six tested MAS exceeds the matched single-agent anchor on benchmark-balanced average accuracy." The other five trail by 2.56–11.29 points and "occupy more expensive accuracy-cost trade-offs." The benchmarks were 10 reasoning, coding and tool-use tasks plus GAIA. — [arXiv 2606.05670](https://arxiv.org/abs/2606.05670)
- **"Rethinking the Value of Multi-Agent Workflow: A Strong Single Agent Baseline" (Xu et al.; arXiv 18 Jan 2026):** "a single agent can reach the performance of homogeneous workflows with an efficiency advantage from KV cache reuse", and can match an automatically optimized heterogeneous workflow. Seven benchmarks, including coding. Exact numbers were not in the abstract. — [arXiv 2601.12307](https://arxiv.org/abs/2601.12307)
- **Token and dollar overheads:**
  - Agents use about 4× the tokens of chat, and MAS about 15×. — [Anthropic multi-agent research](https://www.anthropic.com/engineering/multi-agent-research-system)
  - SWE-agent is 8–13× the cost of RAG. — [SWE-agent HTML](https://arxiv.org/html/2405.15793)
  - Planner + generator + evaluator: $200 / 6 hr vs solo $9 / 20 min. After simplification: $124.70 / 3 hr 50 min, of which the builder alone was $71.08 / 2 hr 7 min. — [Anthropic harness design](https://www.anthropic.com/engineering/harness-design-long-running-apps)
  - Claude Code used 256.9M input tokens vs Terminus 2's 3.9M for Opus 4.5 on Terminal-Bench 2.0, with a lower score. — [Terminal-Bench HTML](https://arxiv.org/html/2601.11868)
- **More compute per step can hurt under time budgets:** xhigh reasoning everywhere scored 53.9% vs high at 63.6% on Terminal-Bench 2.0 (GPT-5.2-Codex), because of timeouts. — [LangChain](https://www.langchain.com/blog/improving-deep-agents-with-harness-engineering)
- **Verification rounds without new signal:**
  - A critic-enhanced summary gave no solve-rate gain and longer trajectories. — [JetBrains 2508.21433](https://arxiv.org/html/2508.21433)
  - The Anthropic evaluator "talk[ed] itself into deciding they weren't a big deal and approve[d] the work anyway" until tuned, and is "unnecessary overhead" for tasks within the model's solo capability. — [Anthropic harness design](https://www.anthropic.com/engineering/harness-design-long-running-apps)
  - Devin Review loops "through multiple code-review cycles, finding new bugs each time (which isn't always great since it can take a while)". — [Cognition 2026](https://cognition.com/blog/multi-agents-working)
  - A verifier "is not a silver bullet"; FC3 verification failures persist even in systems with explicit verifiers. — [MAST v3](https://arxiv.org/html/2503.13657v3)
- **Verification that does help (counter-evidence, keep for balance):**
  - LangChain's self-verification prompt and pre-completion checklist were among the changes behind +13.7 points in a single-agent harness. — [LangChain](https://www.langchain.com/blog/improving-deep-agents-with-harness-engineering)
  - MAST's "high-level task objective verification step" gave +15.6 points on ChatDev ProgramDev. — [MAST v3](https://arxiv.org/html/2503.13657v3)
  - Centralized validation cut error amplification from 17.2× to 4.4× relative to independent agents. — [arXiv 2512.08296](https://arxiv.org/html/2512.08296)
  - SWE-agent's lint-on-edit guardrail was worth +3.0 points. — [SWE-agent HTML](https://arxiv.org/html/2405.15793)
  - Claude 3.7 Sonnet gained 63.7% → 70.3% from parallel attempts plus regression-test rejection plus a scorer. — [Anthropic Claude 3.7 Sonnet](https://www.anthropic.com/news/claude-3-7-sonnet)
- **Error compounding:**
  - "The autonomous nature of agents means higher costs, and the potential for compounding errors." — [Anthropic, Building effective agents, Dec 2024, OLDER EVIDENCE](https://www.anthropic.com/engineering/building-effective-agents)
  - Small per-step accuracy differences compound over horizon length, and models self-condition on their own errors. — [arXiv 2509.09677](https://arxiv.org/abs/2509.09677)
  - "An incorrect step can be easily amplified and negatively affect all future decisions made by the agent." — [Agentless, 2024, OLDER EVIDENCE](https://arxiv.org/abs/2407.01489)
- **"AI Agents That Matter" (Kapoor, Stroebl, Siegel, Nadgir, Narayanan; Princeton; 1 Jul 2024) [OLDER EVIDENCE]:** "there is a narrow focus on accuracy without attention to other metrics. As a result, SOTA agents are needlessly complex and costly, and the community has reached mistaken conclusions about the sources of accuracy gains." MAST cites it as the source for "simple baselines like best-of-N". — [arXiv 2407.01502](https://arxiv.org/abs/2407.01502)

### Inferences
- The Google/DeepMind result is the closest controlled analogue to duet's situation. On coding benchmarks (SWE-bench Verified, Terminal-Bench), multi-agent or centralized coordination with matched compute was flat or negative (−2% to −19%). The mechanism named, a fragmented per-agent token budget on sequential, tool-heavy tasks, fits a local executor with a limited context and throughput budget.
- The plain loop already scores 40–43/50, i.e. 80–86%. That is far above the ~45% ceiling beyond which the Google model predicts negative returns from added agents. On that model, duet's extra phases were expected to cost points, not add them.
- A useful rule from the evidence: a verification round is worth its cost when it injects new ground truth (tests run, lint, regression checks, loop detection, a pre-exit checklist) or when the task is beyond the model's solo reach. It is least valuable, and can backfire, when it is an LLM opinion inserted between phases of a task the executor could finish alone. Such opinions add turns, can be lenient or overly picky, and force context rebuilds. Duet's separate reviewer model would need to be judged on which side of that line it falls.
- Wide run-to-run variance (18–39/50) is consistent with Laban's "unreliability" component, which more than doubles when information arrives in pieces. It is also consistent with Google's error-amplification effect, rather than a uniform capability deficit. This is my inference from the pattern and was not measured on duet.

### Gaps
- None of the controlled MAS-vs-single-agent studies found use local, open-weight executors with a frontier planner. The Google study uses API model families. The "Do More Agents Help?" and OneFlow abstracts do not give coding-specific numbers.
- No source isolated the effect of host-imposed refusals or hard pass limits as a variable. The closest are MAST FM-1.5 (termination awareness, 12.4%), FM-3.1 (premature termination, 6.2%), and Anthropic's "every component encodes an assumption" heuristic.
- I did not locate a primary, peer-reviewed measurement of LLM code-review false-positive rates driving unnecessary rework. The claims above on lenient or looping reviewers are practitioner reports (Anthropic, Cognition).
