# Working hybrid strong-model/cheap-model setups for coding agents, and how reliable LLM-judge and single-run evaluations are

Research date: 2026-09-22. Every source has a date; sources from before 2025 are marked **[OLDER]**. Vendor docs without a visible date are marked "(undated docs, fetched 2026-09-22)". "Inferences" subsections contain my own reasoning. Everything under "Cited Findings" comes from the linked source.

---

## Q1. How do vendors set up a frontier orchestrator with cheaper subagents, and what do they say to delegate?

### Takeaway
Every major coding-agent host now lets you give subagents their own model: Claude Code, Cursor, OpenCode and Codex CLI. OpenCode and Codex CLI can also point at local servers such as Ollama and LM Studio. Across all of them the documented pattern is the same. The strong model keeps the main loop and the user conversation. It hands off bounded side tasks, mostly read-only search and exploration, to a subagent that runs in its own fresh context and returns only a summary. No vendor documents a pattern where a cheap or local model runs every turn of the main loop while a frontier model only plans.

### Cited Findings

**Claude Code (Anthropic), subagents** (undated docs, fetched 2026-09-22; they reference model IDs up to `claude-opus-5-5` and Claude Code v2.1.257, so they are current)
- "Each subagent runs in its own context window with a custom system prompt, specific tool access, and independent permissions." — [Claude Code docs: Subagents](https://code.claude.com/docs/en/sub-agents)
- Stated purpose: "Use one when a side task would flood your main conversation with search results, logs, or file contents you won't reference again: the subagent does that work in its own context and returns only the summary." — [Claude Code docs: Subagents](https://code.claude.com/docs/en/sub-agents)
- A non-fork subagent starts with only its own system prompt, the delegation/task message, CLAUDE.md files (Explore and Plan are exceptions), a git status snapshot and any preloaded skills. It does **not** get the conversation history, previously invoked skills or files that were already read. — [Claude Code docs: Subagents](https://code.claude.com/docs/en/sub-agents)
- Model selection: the `model` frontmatter accepts `sonnet`, `opus`, `haiku`, `fable`, a full model ID, or `inherit`. The model is resolved in this order: (1) the per-invocation `model` parameter, (2) the subagent's frontmatter, (3) the `CLAUDE_CODE_SUBAGENT_MODEL` env var, (4) the main conversation's model. `CLAUDE_CODE_SUBAGENT_MODEL_FORCE=1` (v2.1.257+) forces every subagent onto one model. — [Claude Code docs: Subagents](https://code.claude.com/docs/en/sub-agents)
- Built-ins as currently documented:
  - **Explore** is read-only. It "inherits from main conversation, capped at Opus on Claude API" and skips CLAUDE.md and git status to stay "fast and inexpensive".
  - **Plan** is read-only and inherits the main model.
  - **General-purpose** has all tools and uses `CLAUDE_CODE_SUBAGENT_MODEL` if set, otherwise the main model.
  - The docs say a user- or project-level subagent named `Explore` overrides the built-in, "so define one with `model: haiku` to keep exploration on a lower-cost model."
  - — [Claude Code docs: Subagents](https://code.claude.com/docs/en/sub-agents)
- Stated benefits: "Control costs by routing tasks to faster, cheaper models like Haiku" and "Preserve context by keeping exploration and implementation out of your main conversation." — [Claude Code docs: Subagents](https://code.claude.com/docs/en/sub-agents)
- Limits:
  - Nesting is allowed up to 3 layers below the main conversation.
  - The default concurrent-subagent limit is 20.
  - Built-in Explore and Plan "are one-shot and return no agent ID, so Claude can't resume them".
  - Subagent output is scanned for instruction-shaped text before the main agent reads it.
  - — [Claude Code docs: Subagents](https://code.claude.com/docs/en/sub-agents)
- Anthropic (2025-09-29) on the sub-agent pattern: each subagent "might explore extensively" but "returns only a condensed, distilled summary of its work (often 1,000-2,000 tokens)"; "the detailed search context remains isolated within sub-agents, while the lead agent focuses on synthesizing and analyzing the results." — [Anthropic, Effective context engineering for AI agents (2025-09-29)](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)

**Anthropic on Haiku 4.5 as a sub-agent model** (2025-10-15)
- "Sonnet 4.5 can break down a complex problem into multi-step plans, then orchestrate a team of multiple Haiku 4.5s to complete subtasks in parallel." — [Anthropic, Introducing Claude Haiku 4.5 (2025-10-15)](https://www.anthropic.com/news/claude-haiku-4-5)

**Anthropic advisor tool: the inverse hybrid** (2026-04-09, API beta)
- The cheaper model (Sonnet or Haiku) is the **executor** and runs the whole task with tools. Opus is an **advisor** the executor consults mid-task: "Opus accesses the shared context and returns a plan, a correction, or a stop signal, and the executor resumes. The advisor never calls tools or produces user-facing output." A consultation typically produces "400-700 text tokens". `max_uses` caps advisor calls, and advisor tokens are reported separately. — [Claude blog, The advisor strategy (2026-04-09)](https://claude.com/blog/the-advisor-strategy); tool reference: [Claude Platform Docs, Advisor tool](https://platform.claude.com/docs/en/agents-and-tools/tool-use/advisor-tool) (docs page not fetched; existence confirmed via search)

**Cursor** (undated docs, fetched 2026-09-22)
- Built-in subagents: Explore ("Searches and analyzes codebases"), Bash and Browser. "The explore subagent uses a faster model by default. This enables running 10 parallel searches in the time a single main-agent search would take." — [Cursor Docs: Subagents](https://cursor.com/docs/subagents)
- The `model` field takes `inherit` (the default) or a specific model ID such as `composer-2` or `gpt-5.6-sol`. The parent "only sees the final summary"; "Faster models cost less. Isolating token-heavy work in subagents reduces overall cost." — [Cursor Docs: Subagents](https://cursor.com/docs/subagents)
- May 2026: users can pick a model for Explore subagents, inherit the parent's model, or disable Explore. — [ModelsWar changelog aggregator (May 2026)](https://modelswar.com/change/cursor-adds-model-selection-controls-for-explore-subagents-and-async-multitasking-1634/) (secondary source)

**OpenCode** (undated docs, fetched 2026-09-22)
- Per-agent override: "Use the `model` config to override the model for this agent. Useful for using different models optimized for different tasks." The docs' example is "a faster model for planning, a more capable model for implementation". This is the **opposite** direction to Claude Code's delegation of exploration. — [OpenCode docs: Agents](https://opencode.ai/docs/agents/)
- "If you don't specify a model, primary agents use the model globally configured while subagents will use the model of the primary agent that invoked the subagent." — [OpenCode docs: Agents](https://opencode.ai/docs/agents/)
- Local providers use `@ai-sdk/openai-compatible` with a `baseURL`:
  - Ollama: `http://localhost:11434/v1`
  - LM Studio: `http://127.0.0.1:1234/v1`
  - llama.cpp `llama-server`: `http://127.0.0.1:8080/v1`, with an example `qwen3-coder:a3b` entry set to `limit.context: 128000`, `output: 65536`
  - Guidance: "if tool calls aren't working, try increasing `num_ctx` in Ollama. Start around 16k - 32k", and choose models "with strong tool-calling support (for example, a Qwen-Coder or DeepSeek-Coder variant)."
  - — [OpenCode docs: Providers](https://opencode.ai/docs/providers/)

**OpenAI Codex CLI** (undated docs, fetched 2026-09-22)
- "Codex can run against a local 'open source' provider such as Ollama or LM Studio when you pass `--oss`." The provider is set with `oss_provider = "ollama"` (or `"lmstudio"`). If it is unset, the interactive CLI prompts you to choose and `codex exec` exits with an error. Custom `[model_providers.<id>]` entries take a `base_url` (e.g. `http://localhost:11434/v1`). Named profiles (`codex --profile <name>`) overlay a different model or reasoning effort. — [OpenAI Codex docs: Advanced config](https://learn.chatgpt.com/docs/config-file/config-advanced)
- Default `--oss` model is `gpt-oss:20b`. — [Yunosuke Naito, Run Codex CLI with Ollama or LM Studio using --oss](https://ynaito.dev/en/writing/codex-cli-local-models-oss/) (third-party; not confirmed on the official page I fetched)
- Codex subagent roles are configured under `[agents]` in `config.toml`. Per-role overrides reportedly include `model`, `model_reasoning_effort`, `sandbox_mode` and `developer_instructions`. — [Codex Knowledge Base, Custom agent definitions (2026-04-27)](https://codex.danielvaughan.com/2026/04/27/codex-cli-custom-agent-definitions-toml-specialised-subagents/) (third-party); a GitHub issue asked for exactly this — [openai/codex issue #11795, "Allow configuring subagent model and reasoning_effort in config"](https://github.com/openai/codex/issues/11795)

### Inferences
- All the documented setups delegate **downward in capability for read-heavy, low-commitment work**: search, exploration, bash batches, summarizing. The frontier model keeps the turns that make decisions and edits. Anthropic's advisor tool is the one documented pattern where a cheaper model runs every turn. Even there the executor is Sonnet or Haiku, not a local open-weight model, and the frontier model steps in *on demand*, not just once at plan time.
- For a "≥80% local" goal, the closest documented shapes are: (a) a frontier main loop with local subagents doing exploration/search/test-running (Claude Code cannot do this natively because its subagents are Claude models; OpenCode and Codex can mix a local provider per agent), and (b) an advisor-style local executor that calls a frontier advisor at decision points. Neither shape has published evidence of reaching frontier parity with a *local* model.
- The OpenCode docs example (cheap planner, strong implementer) and the Claude Code/Cursor practice (cheap explorer, strong main agent) point different ways. That means there's no vendor consensus that "frontier plans, cheap executes" is the right split.

### Gaps
- I found no official Cursor documentation naming which "faster model" Explore uses by default, and no official statement that Cursor routes apply/search to cheap models today beyond Explore.
- I could not confirm from the official Codex docs page I fetched that per-role `model` is supported in current Codex (third-party guides say yes; issue #11795 suggests it was once missing).
- Claude Code docs have changed over time; earlier versions reportedly ran Explore on Haiku by default. I found no dated changelog confirming when Explore switched to "inherit". The current docs say inherit.

---

## Q2. What measured results exist for these hybrid setups (cost, quality retained, tokens, fast-apply accuracy)?

### Takeaway
The measurements are few and almost all come from vendors. The best-supported numbers are:
- Anthropic's advisor pattern: Sonnet executor with Opus advisor scored +2.7 pp on SWE-bench Multilingual over Sonnet alone at 11.9% lower cost per task. Haiku with an Opus advisor went from 19.7% to 41.2% on BrowseComp.
- Anthropic's multi-agent research system: +90.2% over single-agent Opus 4 on an internal *research* eval, using about 15× the tokens of chat.
- Aider's 2024 architect/editor split: +5.3 pp over o1-preview alone.
- Fast-apply models claim roughly 96–98% merge accuracy at around 10k tok/s. These are vendor numbers.

I found no independent measurement of token or cost savings from delegating exploration to subagents inside a coding agent.

### Cited Findings

**Advisor pattern (cheap executor, frontier advisor)** — [Claude blog, The advisor strategy (2026-04-09)](https://claude.com/blog/the-advisor-strategy)
- "Sonnet with Opus as an advisor showed a 2.7 percentage point increase on SWE-bench Multilingual over Sonnet alone", "reducing cost per agentic task by 11.9%".
- Secondary sources put the scores at 74.8% (with advisor) vs 72.1% (Sonnet alone), and describe the 11.9% as "less than running Opus solo". The fetched primary text was ambiguous about which baseline the 11.9% saving is measured against. — [MindStudio (2026)](https://www.mindstudio.ai/blog/anthropic-advisor-strategy-opus-sonnet-haiku); [Builder.io (2026)](https://www.builder.io/blog/the-claude-advisor-pattern)
- BrowseComp: Haiku + Opus advisor scored 41.2% vs 19.7% for Haiku alone. It "trails Sonnet solo by 29% in score but costs 85% less per task".
- Terminal-Bench 2.0: the fetched text says Sonnet + Opus advisor "showed improvements" but gave no exact figure in my extract.

**Multi-agent research system (Opus 4 lead + Sonnet 4 subagents)** — [Anthropic Engineering, How we built our multi-agent research system (2025-06-13)](https://www.anthropic.com/engineering/multi-agent-research-system)
- The multi-agent system "outperformed single-agent Claude Opus 4 by 90.2%" on Anthropic's internal research eval. This is a research/browsing task, not coding.
- "agents typically use about 4× more tokens than chat interactions"; "multi-agent systems use about 15× more tokens than chats".
- "token usage by itself explains 80% of the variance" in their browsing eval, and three factors together explain 95%.

**Haiku 4.5 as the cheap model** — [Anthropic, Introducing Claude Haiku 4.5 (2025-10-15)](https://www.anthropic.com/news/claude-haiku-4-5)
- Price is $1/$5 per million input/output tokens. SWE-bench Verified is 73.3%, "averaged over 50 trials, no test-time compute, 128K thinking budget, default sampling parameters, full 500-problem dataset", with bash and string-replace editing tools.
- Customer claims (vendor-selected quotes, not independent):
  - Augment: "achieves 90% of Sonnet 4.5's performance" on its agentic coding eval.
  - Notion: "up to 4-5 times faster than Sonnet 4.5 at a fraction of the cost".
  - GitHub Copilot: "comparable quality to Sonnet 4 but at faster speed".

**Architect/editor split (Aider)** — [Aider blog, Separating code reasoning and editing (2024-09-26)](https://aider.chat/2024/09/26/architect.html) **[OLDER]**
- Pass rates on Aider's code-editing benchmark:

  | Architect | Editor (format) | Pass rate |
  |---|---|---|
  | o1-preview | o1-mini (whole) | 85.0% |
  | o1-preview | DeepSeek (whole) | 85.0% |
  | o1-preview | Claude 3.5 Sonnet (diff) | 82.7% |
  | o1-preview | DeepSeek (diff) | 80.5% |
  | o1-preview | GPT-4o (diff) | 80.5% |
  | o1-preview | none, solo baseline (diff) | 79.7% |
  | Claude 3.5 Sonnet | Claude 3.5 Sonnet (diff) | 80.5% |
  | Claude 3.5 Sonnet | DeepSeek (diff) | 78.9% |
  | GPT-4o | GPT-4o (diff) | 75.2% |
  | o1-mini | DeepSeek (whole) | 71.4% |
  | GPT-4o-mini | GPT-4o-mini (whole) | 60.2% |

- Aider's explanation for the gain: without the split, "the model has to split its attention between solving the coding problem and conforming to the edit format."

**Fast-apply models (a small model merges edits a large model wrote)**
- Cursor, 2024-05-14 **[OLDER]**:
  - A fine-tuned Llama-3-70B runs at "~1000 tokens" per second (~3500 char/s), a "~13x speedup over vanilla inference using Llama-3-70b" and "~9x speedup over our previous GPT-4 speculative edits deployment".
  - On Cursor's eval, llama-3-70b-ft "almost matches claude-3-opus-diff and outperforms gpt-4-turbo and gpt-4o".
  - — [Cursor blog, Near-instant full-file edits (2024-05-14)](https://cursor.com/blog/instant-apply)
- Relace Apply 3, 2025-10-29:
  - "10k tok/s" via speculative decoding and native 256k context.
  - The eval uses "500 randomly sampled production merge requests" with an LLM judge (six categories collapsed to correct/incorrect).
  - Editing a 1,000-line file with Claude Sonnet 4.5 takes "over 100 seconds and costs at least $0.18".
  - Untuned models "still fail around 10% of the time".
  - The blog's own exact accuracy numbers for Apply 3 were not in my extract. — [Relace blog, A Year of Fast Apply (2025-10-29)](https://relace.ai/blog/relace-apply-3)
  - A search summary attributes "~96% accuracy" to Relace Instant Apply. — [Relace docs, Model Overview](https://docs.relace.ai/docs/instant-apply/overview) (page returned 404 when fetched; unverified)
- Morph: "complete merged file at 10,500 tokens per second with 98% accuracy", "~35% faster end-to-end compared to search-and-replace". — [Morph, Fast Apply model page](https://www.morphllm.com/fast-apply-model) (vendor claim, undated, seen via search snippet only; the page returned HTTP 429 on fetch). An aggregator lists Morph v3 Fast at "about 96% apply accuracy". — [Merge.dev gateway page](https://www.merge.dev/gateway/morph-v3-fast-api) (secondary)

### Inferences
- The advisor result is the strongest published evidence for the pattern "cheap model executes every turn, frontier model advises". The gains came from executors that are themselves strong: Sonnet at 72.1% on SWE-bench Multilingual on its own, and Haiku 4.5 at 73.3% on SWE-bench Verified. The evidence does not carry over to a much weaker local executor.
- Aider's split helped when the **editor was at least as capable as the task needed**. Swapping a weaker editor under a fixed architect lowered scores: Sonnet+Sonnet scored 80.5% and Sonnet+DeepSeek scored 78.9%, a 1.6 pp drop, although this is within the noise levels in Q5.
- Fast-apply numbers show a small model can do the *mechanical* part of an edit, merging a snippet into a file, at about 96–98% accuracy by vendor evals. That supports delegating the **apply** step, not the reasoning or execution loop.
- Anthropic's 15× token multiple for multi-agent systems means delegation *raises* total tokens. Cost only falls if the extra tokens run on a much cheaper (or local) model and the frontier model's context stays small.

### Gaps
- I found no independent (non-vendor) measurement of cost or quality for Claude Code, Cursor or OpenCode subagent delegation on a coding benchmark, and no published numbers for token savings from an Explore subagent in coding.
- I found no published result for a *local open-weight* executor paired with a frontier planner or advisor reaching frontier parity on SWE-bench-style tasks.
- Terminal-Bench 2.0 advisor figures, and the exact baseline for the 11.9% cost saving, were not clear in the primary text I fetched.
- Morph's and Relace's own benchmark pages could not be fetched (HTTP 429 and 404), so their accuracy numbers are from snippets or aggregators.

---

## Q3. What are the known limits of subagent delegation?

### Takeaway
The sources name four documented costs: (1) information loss, because the subagent starts with a clean context and the parent sees only a summary; (2) more total tokens (about N× for N parallel subagents, about 15× for multi-agent vs chat); (3) startup overhead as each subagent rebuilds its own context; and (4) conflicting implicit decisions when subagents act (write code) rather than answer questions. Anthropic and Cognition both say coding is a poor fit for parallel multi-agent work.

### Cited Findings
- "Subagents start with a clean context. The parent agent includes relevant information in the prompt." "Running five subagents in parallel uses roughly five times the tokens." Each subagent pays startup overhead gathering its own context. — [Cursor Docs: Subagents](https://cursor.com/docs/subagents) (undated, fetched 2026-09-22)
- Claude Code subagents do not get conversation history, earlier skills or files already read. Only the task message and some context files are passed in. — [Claude Code docs: Subagents](https://code.claude.com/docs/en/sub-agents)
- "some domains that require all agents to share the same context or involve many dependencies between agents are not a good fit for multi-agent systems today. For instance, most coding tasks involve fewer truly parallelizable tasks than research." — [Anthropic Engineering (2025-06-13)](https://www.anthropic.com/engineering/multi-agent-research-system)
- Anthropic's mitigation for loss through summaries: "Subagents call tools to store their work in external systems, then pass lightweight references back to the coordinator. This prevents information loss during multi-stage processing." — [Anthropic Engineering (2025-06-13)](https://www.anthropic.com/engineering/multi-agent-research-system)
- Cognition (Walden Yan):
  - "Share context, and share full agent traces, not just individual messages"; "Actions carry implicit decisions, and conflicting decisions carry bad results".
  - On Claude Code: "it never does work in parallel with the subtask agent, and the subtask agent is usually only tasked with answering a question, not writing any code."
  - On older edit-apply splits: "the large models output markdown explanations of code edits and then fed these markdown explanations to small models to actually rewrite the files", and "these systems would still be very faulty."
  - — [Cognition, Don't Build Multi-Agents (2025-06-12)](https://cognition.com/blog/dont-build-multi-agents)
- Cognition's suggested alternative for long tasks is a dedicated model "whose key purpose is to compress a history of actions & conversation into key details, events, and decisions." — [Cognition (2025-06-12)](https://cognition.com/blog/dont-build-multi-agents)

### Inferences
- A "frontier plans once, local model executes every turn" design is close to the failure mode Cognition describes: the plan is a summary, and the executor makes many implicit decisions the planner never sees. The documented working patterns avoid this in two ways. Either the context-holding model makes the decisions (a frontier main loop with question-answering subagents), or the executor holds the full context and escalates (the advisor pattern, where the advisor "accesses the shared context").
- Cognition's "still very faulty" verdict on markdown-to-small-model edit splits (June 2025) conflicts with the vendor fast-apply accuracy claims of about 96–98%. The two most likely differ in input format: prose explanations vs code snippets with "keep existing code" markers. I could not verify the snippet format itself (see Gaps).

### Gaps
- I found no quantitative study of how much information is lost in subagent summaries (for example, the rate at which facts the parent needs are dropped).
- The Relace/Morph input format (code snippet plus unchanged-code markers) is my understanding of fast-apply APIs. The Relace docs page 404'd, so I could not cite it directly.

---

## Q4. How reliable are LLM judges (repeat-run variance, biases, agreement with humans), and how many judgments do you need?

### Takeaway
Strong LLM judges agree with humans about as often as humans agree with each other (about 80–85% on non-tied pairwise votes). They are also self-inconsistent across repeated runs, sensitive to answer position, can be swayed by verbosity (depending on the judge) and favor their own outputs. Statistical guidance (Miller 2024) says to report standard errors, use paired comparisons and resample to reduce within-item variance. Detecting small differences takes hundreds to about 1,000 items. By that guidance, one judgment per configuration cannot resolve a 1-point difference on a rubric where re-scoring the same file swings by about 7 points.

### Cited Findings

**Zheng et al., "Judging LLM-as-a-Judge with MT-Bench and Chatbot Arena"** (arXiv 2306.05685; v1 2023-06-09, final 2023-12-24; NeurIPS 2023) **[OLDER]** — [arXiv](https://arxiv.org/abs/2306.05685)
- Agreement: on MT-Bench first-turn, non-tie votes (setup S2), GPT-4 pairwise vs humans was **85%**, compared with **81%** between humans. With ties and inconsistent votes counted (S1) the figures are 66% vs 63%. The random baselines are 50% (S2) and 33% (S1).
- Position bias (Table 2, swapping answer order):

  | Judge | Consistent | Favored first answer |
  |---|---|---|
  | GPT-4 | 65.0% | 30.0% |
  | GPT-3.5 | 46.2% | 50.0% |
  | Claude-v1 | 23.8% | 75.0% |

- Few-shot prompting raised GPT-4's consistency from 65.0% to 77.5%, but made calls 4× more expensive, and "high consistency may not imply high accuracy".
- Verbosity ("repetitive list" attack, 23 answers): failure rates were Claude-v1 91.3%, GPT-3.5 91.3%, GPT-4 8.7%.
- Self-enhancement: "GPT-4 favors itself with a 10% higher win rate; Claude-v1 favors itself with a 25% higher win rate"; GPT-3.5 does not favor itself.
- Math grading, where failure means GPT-4 calls an incorrect answer correct: default prompt 14/20, chain-of-thought 6/20, reference-guided 3/20 (70% → 15%).

**Panickssery et al., "LLM Evaluators Recognize and Favor Their Own Generations"** (NeurIPS 2024; arXiv 2404.13076) **[OLDER]**
- GPT-4 and Llama 2 have non-trivial accuracy at recognizing their own text, and after fine-tuning there is a linear correlation between self-recognition and the strength of self-preference bias. — [arXiv 2404.13076](https://arxiv.org/abs/2404.13076); [NeurIPS 2024 proceedings](https://proceedings.neurips.cc/paper_files/paper/2024/hash/7f1f0218e45f5414c79c0679633e47bc-Abstract-Conference.html)

**Haldar & Hockenmaier, "Rating Roulette: Self-Inconsistency in LLM-As-A-Judge Frameworks"** (2025-10-31) — [arXiv 2510.27106](https://arxiv.org/html/2510.27106v1)
- Self-consistency across repeated runs on identical inputs (Krippendorff's α, 3 runs):

  | Task | Llama 3.1 | DeepSeek-R1 | Qwen-3 |
  |---|---|---|---|
  | SummaC (binary) | 0.326 | 0.628 | 0.788 |
  | MT-Bench (ranking) | 0.265 | 0.507 | 0.563 |

- "Qwen 3 gave the same judgment on all 3 runs for only 61.3% of cases" on MT-Bench.
- SummEval (1–5 Likert): high self-reliability on Coherence and Consistency, "very low self-reliability on Fluency".
- Setting temperature 0 hurt agreement with humans ("reducing variance hurts performance measured by agreement with human judgment").
- Recommendation: aggregate multiple runs (for example, majority vote).

**Norman, Rivera & Hughes, "Reliability without Validity"** (2026-06-17) — [arXiv 2606.19544](https://arxiv.org/html/2606.19544v1)
- Scale: 21 judges from 9 providers, 3 benchmarks, 118 evaluation runs, about 541,000 judgments.
- Agreement with humans on MT-Bench: Cohen's κ 0.376–0.511. Chance correction lowered cohort-mean agreement by 38.6 pp compared with exact match.
- Test-retest reliability: 0.888–0.992 (cohort mean 0.944 on MT-Bench, 0.911 on JudgeBench).
- Position bias ranged from 0.002 to 0.192. Two judges had both reliability >0.95 and position bias >0.10: reproducible but biased.
- Verbosity bias was below 0.011 for all 21 judges, and below 0.005 for 17 of them.
- Proposed minimum protocol: κ as the headline metric, AB+BA position swaps, test-retest over ≥3 runs, ≥2 benchmarks.

**Anthropic, "Demystifying evals for AI agents"** (2026-01-09) — [Anthropic Engineering](https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents)
- "LLM-as-judge graders should be closely calibrated with human experts". "grade each dimension with an isolated LLM-as-judge rather than using one to grade all dimensions". Let the judge return "Unknown" when it lacks information. "You won't know if your graders are working well unless you read the transcripts and grades from many trials."
- Non-determinism: "a task that passed on one eval run might fail on the next". pass^k example: at 75% per-trial success, the chance of passing all 3 trials is (0.75)³ ≈ 42%. Suggested starting set: "20-50 simple tasks drawn from real failures".

**Miller, "Adding Error Bars to Evals: A Statistical Approach to Language Model Evaluations"** (Anthropic; arXiv 2411.00640, 2024-11-01) **[OLDER, but the standard reference]** — [arXiv abstract](https://arxiv.org/abs/2411.00640); [HTML](https://arxiv.org/html/2411.00640)
- Report the standard error: SE = √(Var(s)/n), and the 95% CI is mean ± 1.96·SE (for binary scores, √(p(1−p)/n)).
- Clustered standard errors "can be over 3X larger than naive standard errors" on DROP, RACE-H and MGSM. Report the cluster count.
- Use paired differences: SE_{A−B} = √(SE_A² + SE_B² − 2·SE_A·SE_B·Corr). A paired design is a "'free' reduction in estimator variance".
- Resampling each question K times cuts within-question variance to σ²ᵢ/K. For binary scores, K=2 removes 1/3 of the variance, K=4 removes 1/2 and K=6 removes 5/9. Beyond the point where E[σ²ᵢ]/K ≪ Var(x), more samples help little.
- Power formula: n = (z_{α/2} + z_β)² (ω² + σ²_A/K_A + σ²_B/K_B) / δ². Detecting δ = 3 pp at 80% power and α = 0.05 (ω² = 1/9) needs **n ≈ 969 questions**, hence "new evals should contain at least 1,000 questions".
- Advises against changing temperature to reduce variance unless the goal is to study that temperature.

### Inferences (applied to the project's setup: one artifact per configuration per round, a 50-point rubric, a judge that swings about 7 points on an unchanged file, and a target of ±1 point parity)
- **Estimating judge noise from the 7-point swing.** Assume scores are roughly normal. The expected range of m repeat scores is d₂(m)·σ, where d₂ = 1.128 for m=2, 1.693 for 3, 2.326 for 5 and 3.078 for 10. A 7-point range therefore implies:

  | Re-scorings behind the 7-point swing | Implied judge SD σ_j |
  |---|---|
  | 2 | ≈ 6.2 |
  | 3 | ≈ 4.1 |
  | 5 | ≈ 3.0 |
  | 10 | ≈ 2.3 |

  I use σ_j ≈ 3 as the central case. This is my own calculation, not a cited result.
- **One judgment per arm.** Comparing two configurations from one judgment each, judge noise alone gives the difference an SD of σ_j·√2 ≈ 4.2 points. The 95% CI half-width is 1.96·4.2 ≈ **±8.3 points**. The smallest difference detectable at 80% power is 2.80·σ_j·√2 ≈ **9–16 points** for σ_j = 2.3–4.1 (about 25 points if σ_j ≈ 6.2). A ±1-point parity claim sits well inside judge noise, before counting any run-to-run variance in the artifact itself.
- **Judgments needed to detect a 1-point difference.** Two-sided α = 0.05, 80% power, judge noise only, unpaired. Per-arm n = 2·(1.96+0.84)²·σ_j²/δ² ≈ 15.7·σ_j²:

  | Implied σ_j | Judgments per configuration |
  |---|---|
  | 2.3 | ≈ 83 |
  | 3.0 | ≈ 141 |
  | 4.1 | ≈ 264 |
  | 6.2 | ≈ 600 |

  This is a lower bound, because it assumes the artifacts themselves don't vary.
- **Equivalence testing.** To *show* parity within ±1 (a TOST equivalence test, α = 0.05, 80% power, true difference 0), you need n ≈ 17.1·σ_j² per arm, which is about 154 at σ_j = 3.
- **Where more judging stops helping.** Re-judging the same artifact K times only shrinks the judge term. The variance of a configuration's mean is σ_gen²/n_artifacts + σ_j²/(n_artifacts·K), which is Miller's formula with ω² ↔ artifact-to-artifact variance. With one artifact per configuration, the artifact-to-artifact variance σ_gen² cannot be estimated at all, and no amount of re-judging removes it. Following Miller, you need several independent agent runs (artifacts) per configuration, a paired design (same tasks, same judge prompt, ideally a pairwise AB+BA comparison of the two configurations' artifacts) and reported standard errors.
- **Bias risks specific to this setup.** Self-preference (Zheng: +10–25% win rate; Panickssery) matters if the judge is the same model family as the frontier configuration being matched. Position bias matters for any pairwise judging and needs AB+BA swaps. Verbosity bias varies a lot by judge: severe in 2023-era judges, below 0.011 in the 21 judges tested in 2026.

### Gaps
- I found no study that reports test-retest SD specifically for **50-point, multi-criterion code-quality rubrics**. The literature mostly covers pairwise verdicts, Likert 1–5, or binary labels. The σ_j figures above are derived from the project's own 7-point observation, not from a published source.
- I found no published sample-size table for LLM-judge rubric scores in particular. The calculations above apply standard power formulas (Miller 2024) to assumed σ values.

---

## Q5. How much do agentic benchmark results vary between repeated runs, and what does that mean for N=1 comparisons?

### Takeaway
A single run of an agentic coding benchmark has run-to-run spread of several points. On SWE-bench Verified, single-run pass@1 swings by 2.2–6.0 pp depending on which run you pick, with SD above 1.5 pp even at temperature 0. Changing infrastructure alone moves Terminal-Bench 2.0 by 6 pp, and Anthropic says differences under 3 pp "deserve skepticism". Most adjacent Terminal-Bench leaderboard ranks are statistically indistinguishable. N=1 comparisons therefore can't separate small effects.

### Cited Findings
- Bjarnason, Silva & Monperrus, "On Randomness in Agentic Evals" (v1 2026-02-06, v3 2026-03-25) — [arXiv 2602.07150](https://arxiv.org/abs/2602.07150):
  - Setup: 60,000 SWE-Bench-Verified trajectories, 3 models, 2 scaffolds.
  - "single-run pass@1 estimates vary by 2.2 to 6.0 percentage points depending on which run is selected, with standard deviations exceeding 1.5 percentage points even at temperature 0."
  - "Reported improvements of 2--3 percentage points may reflect evaluation noise rather than genuine algorithmic progress."
  - Recommendations: estimate pass@1 from multiple independent runs, use power analysis to choose the number of runs, and report pass@k and pass^k.
- Anthropic, "Quantifying infrastructure noise in agentic coding evals" (2026-02-05) — [Anthropic Engineering](https://www.anthropic.com/engineering/infrastructure-noise):
  - Terminal-Bench 2.0 across six resource configurations: a "6 percentage points (p < 0.01)" gap between the most and least resourced setups.
  - Infrastructure error rate: 5.8% at strict enforcement vs 2.1% at 3× headroom (p < 0.001). About 6% of tasks failed from pod errors under strict limits, vs 0.5% uncapped.
  - SWE-bench crossover (227 problems × 10 samples): 1.54 pp higher at 5× resources than at 1×.
  - "Leaderboard differences below 3 percentage points deserve skepticism until the eval configuration is documented and matched."
- Terminal-Bench 2.0 leaderboard noise analysis (community post, not peer-reviewed; author "nateg551015", 2026-08-05) — [LessWrong, Terminal-Bench leaderboard rankings: luck or skill?](https://www.lesswrong.com/posts/GiPmLmmbT6DyrwYkH/terminal-bench-leaderboard-rankings-luck-or-skill):
  - Setup: 89 tasks, at least 5 tries per task, 325 agent pairs, task-clustered bootstrap with Benjamini-Hochberg correction.
  - "23% were statistically equivalent" across the 325 pairs, and "24/25" adjacent leaderboard pairs differ by "essentially statistical noise". Ranks 5–13 spanned only 67% to 62%.
  - Scaffold effect: "Claude Haiku 4.5 -- two scaffolds, 22 points span" vs "Gemini 3 Pro -- four scaffolds, 4 point span".
  - The post notes that trials within a task are correlated, so treating them as independent understates CI width by about 2×.
- Haiku 4.5's official SWE-bench Verified score (73.3%) is averaged over 50 trials. Vendors themselves average many runs for headline numbers. — [Anthropic (2025-10-15)](https://www.anthropic.com/news/claude-haiku-4-5)
- Miller: clustered SEs can be more than 3× naive SEs, so repeated trials on the same task must be analyzed with task-clustered SEs. — [arXiv 2411.00640 (2024-11-01)](https://arxiv.org/html/2411.00640) **[OLDER]**

### Inferences
- On benchmarks with hundreds of tasks, a single run's pass@1 carries about 1.5+ pp SD. The project's "one artifact per configuration per round" is effectively **N=1 over tasks as well as runs**, so its noise floor is far higher than a 500-task benchmark's. A single agent run on one goal can land anywhere in its outcome distribution, and a judge swing of about 7 points is added on top.
- Scaffold and infrastructure effects (22 points across scaffolds for Haiku 4.5, 6 pp from resource limits) are as large as or larger than model differences. Parity comparisons between "frontier" and "≥80% local" configurations must hold the harness, resources and retry policy fixed, or the difference will reflect infrastructure rather than model quality. This matches the project's own history of infrastructure failures being recorded as work outcomes.
- Practical implication, which is my inference: a defensible ±1-point parity claim needs (a) several independent agent runs per configuration per goal, (b) several goals or tasks, (c) repeated or pairwise, position-swapped judging, and (d) a CI or equivalence test on the paired difference. Alternatively, move the success criterion toward deterministic signals (tests passing, gates green) and measure those over multiple runs.

### Gaps
- I found no published run-to-run variance figures for long-horizon *single-goal* agent runs scored by an LLM rubric (as opposed to pass/fail benchmarks).
- The Terminal-Bench leaderboard analysis is a community post. I found no peer-reviewed equivalent for Terminal-Bench 2.0 specifically, though Anthropic's infrastructure-noise post and arXiv 2602.07150 support the same conclusion for Terminal-Bench and SWE-bench respectively.
