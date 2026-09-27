# Duet compared with similar solutions (2026-09-27)

Companion to [Enterprise LLM privacy for banks](Enterprise%20LLM%20privacy%20for%20banks.md) (2026-09-25), which covers vendor contracts, DLP,
TEEs and bank practice. This report covers the systems that most resemble Duet. Sources were
collected by three web-research passes on 2026-09-27. Figures are as the sources report them; most
are from abstracts and have not been re-derived. Where a source is inconsistent, that is noted.

## Bottom line

No shipped coding agent does what Duet does. The idea itself (a local model reads the private data
while a cloud model plans and decides) is well established in research. In September 2026 it also
reached one consumer product, Perplexity Hybrid Compute, which is not a coding agent. The coding
tools that come closest are redaction hooks and proxies, plus a few tiny open-source projects. Each
covers one or two of Duet's layers, and none measures leaks end to end on real coding tasks. The
closest research systems (PlanTwin, SlotGuard, PAAC, SecureClaw, LLM-Redactor) test on synthetic,
QA or partial-repository tasks.

Duet leads on how much of the boundary it covers and on measuring it. It trails on the channel its
local answers open, on speed, on quality on the hardest tasks, and on maturity.

## 1. Same split, not coding

| System | Split | Result | Source |
|---|---|---|---|
| Perplexity Hybrid Compute (1 Sep 2026) | Cloud models search, plan and reason. Private files go to a local model on the Mac, and an on-device 0.6B classifier keeps, masks, refuses or asks for consent | Product for Pro, Max and Enterprise on Apple Silicon; the classifier is open-sourced; no audit log documented | https://www.marktechpost.com/2026/09/01/perplexity-releases-hybrid-compute-on-mac-cloud-agents-orchestrate-down-to-a-local-model-gated-on-device/ · https://x.com/perplexity_ai/status/2094803534927770022 |
| Wells Fargo Fargo | A local SLM tokenizes PII; Gemini Flash sees de-identified text | 245M interactions in 2024; customer chat | see the banks report |
| Minions / MinionS → OpenJarvis (Stanford) | The local model reads long context and the cloud model directs | 97.9% of GPT-4o at 5.7× lower cloud cost; "we do not address privacy"; retrospective: "coding tasks still favor the cloud" | https://arxiv.org/abs/2502.15964 · https://hazyresearch.stanford.edu/blog/2026-05-15-minions-to-openjarvis-retrospective |
| Secure Minions | Minions inside an H100 TEE | <1% overhead for 32B+; not third-party audited | https://hazyresearch.stanford.edu/blog/2025-05-12-security |
| PAPILLON | A local model rewrites queries for the cloud model | 85.5% quality, 7.5% leakage (Llama-3.1-8B); 1B model: 58.0% quality | https://arxiv.org/abs/2410.17127 |
| PAAC (May 2026) | Typed placeholder tokens go to the cloud; a regex registry restores them; tools run on the device | τ²-Retail strictest: 58.5% accuracy, 16.7% leakage (PAPILLON: 39.8%, 98.4%); τ² and GAIA, no coding | https://arxiv.org/abs/2605.08646 |
| PrivScope (May 2026) | On-device intermediary for hybrid agents | Profile leakage 17.7% → 0.0%; re-identification 64.3% → 23.1% (medical booking) | https://arxiv.org/abs/2605.16630 |
| DR-SL (14 Sep 2026) | Locally verified de-identify/restore | 67.5% released automatically at 0 measured leakage | https://arxiv.org/abs/2609.14883 |
| Google, Hartmann et al. (2024) | Gemini Nano replaces entities before asking Ultra | Entity leak 1.2%, but mapping leak 53.8% | https://arxiv.org/abs/2404.01041 |
| Hide and Seek (2023) | A local model anonymizes and a second local model restores | Earliest reversible-placeholder design | https://arxiv.org/abs/2309.03057 |

Other items: ConfusionPrompt (https://arxiv.org/abs/2401.00870), CoGenesis (https://arxiv.org/abs/2403.03129),
Privacy-R1 (https://arxiv.org/abs/2510.16054), Need to Know / DelegateCI-Bench (https://arxiv.org/abs/2606.04067),
Beyond Direct Identifiers (https://arxiv.org/abs/2608.09140), P2Skill (https://arxiv.org/abs/2608.14094),
Minim (https://arxiv.org/abs/2606.13949).

## 2. Coding and agentic research closest to Duet

- **PlanTwin** (Mar 2026), https://arxiv.org/html/2603.18377.
  - Setup: a cloud planner sees only a de-identified typed graph of the environment. A local gatekeeper enforces capability whitelists and per-object disclosure budgets.
  - Tasks: 60 synthetic ones (coding assistant, debugging, DevOps, database), with planted keys and connection strings.
  - Results: no sensitive item disclosed, under 2.2% utility loss, 16.5% leakage for a PII-redaction baseline. **Without budgets, 94.1% of objects were re-identifiable from their structure.**
  - Difference from Duet: the cloud only chooses plans; it never writes code against a real repository.
- **SlotGuard** (Jul 2026), https://arxiv.org/html/2607.17147.
  - Setup: typed, format-preserving slots in agent transcripts. Substitution is deterministic; a local model only advises.
  - Results: 0.0% of 852 planted credentials leaked; task success 89.5–100% against 93.5–100% raw (generic redaction: 2.5%).
  - Tasks: a TheAgentCompany subset. The authors say it cannot stop semantic inference.
- **SecureClaw** (Jun 2026), https://arxiv.org/html/2606.09549. The planner sees opaque handles plus bounded summaries from a fixed operator (no LLM), and writes go through preview-then-commit. 3.23% leakage on AgentLeak; no coding tasks.
- **LLM-Redactor** (Apr 2026), https://arxiv.org/abs/2604.12064.
  - The only published measurement of proprietary-code leakage with a local model (3B) in the loop.
  - Placeholders leaked 7.0% of PII, 16.2% of secrets and 42.8% of code; the best combination still leaked 31.3% of code (abstract).
  - Caveat: one extraction of its Table 4 gave 0.123 for the same combination, so check before quoting.
- **Tool-level and benchmark work:**
  - FLOWSEAL (IFC at the tool layer, 52.2% → 0.5%): https://arxiv.org/abs/2609.14003
  - ToolMinimize (81–88% of tool calls carry unneeded private data): https://arxiv.org/abs/2608.24957
  - CodeCloak (~40% code leak): https://arxiv.org/abs/2404.09066
  - NOIR (noised code embeddings, HumanEval −1.77%): https://arxiv.org/abs/2601.16354
  - Robustness on obfuscated code (~90% Pass@1 kept): https://arxiv.org/abs/2609.04220
  - Twin Agent (SWE-bench Lite): https://arxiv.org/abs/2607.19595
- **Leak benchmarks:**
  - AgentDAM (GPT-4o 35.4% → 8.5%): https://arxiv.org/html/2503.09780
  - PrivacyLens (GPT-4 25.68%): https://proceedings.neurips.cc/paper_files/paper/2024/file/a2a7e58309d5190082390ff10ff3b2b8-Paper-Datasets_and_Benchmarks_Track.pdf
  - AgentLeak (inter-agent messages 68.8% against 27.2% for outputs): https://arxiv.org/abs/2602.11510
  - ToolPrivacyBench: https://arxiv.org/abs/2606.28061
  - Korea/Singapore AI Safety Institutes study: https://arxiv.org/abs/2606.17114
- **Grok Build wire analysis** (Jul 2026), https://www.developersdigest.tech/blog/grok-cli-wire-level-analysis. An independent mitmproxy test found canary `.env` secrets sent verbatim and the whole repository (5.10 GiB) uploaded. This is the only public canary-plus-proxy test of a coding agent found.

None of these runs a full coding agent on real repository tasks with a local reader and end-to-end
canary and proxy leak measurement. That combination is the experiment Duet runs.

## 3. Add-ons and features that protect coding agents

| Tool | Mechanism | What the cloud model still sees | Tool/terminal output | Open source | Source |
|---|---|---|---|---|---|
| GitGuardian ggshield AI Hooks | Scans prompts, pre-tool and post-tool (500+ secret types); since 25 Sep 2026 it withholds secret-bearing output on Claude Code, Codex and Mistral Vibe | Everything except withheld secret-bearing output; not reversible | Yes on Claude Code, Codex and Vibe; not on Cursor, VS Code or Copilot CLI | CLI MIT; detection runs on GitGuardian's servers | https://docs.gitguardian.com/releases/saas/2026/09/25/changelog |
| CrowdStrike AIDR (ex-Pangea) | Claude Code hooks; redacts tool I/O | Unredacted user prompts; policy runs in AIDR's cloud | Tool I/O, no subagents | No | https://aidr-docs.crowdstrike.com/docs/aidr/collectors/agentic/claude-code |
| PrivAiTe | Proxy with Presidio plus OpenAI Privacy Filter; reversible `<PERSON_1>` (secrets destroyed) | Everything not detected | Whole API request, including tool arguments | BSD-3 | https://github.com/crp4222/PrivAiTe |
| llm-redact-proxy | gitleaks plus Privacy Filter (MLX); reversible and fail-closed | Everything not detected | Whole request | MIT | https://github.com/CupOfGeo/llm-redact-proxy |
| claude-code-privacy-proxy, claude-code-redact, hygienics, agent-guard, redact-hook, cc-redact, mintmcp/agent-security | Regex or gitleaks proxies and hooks; mostly irreversible | Everything not matched | Varies; cc-redact is bypassed by `@.env` | MIT / Apache / AGPL | https://github.com/PavanKalyanV5/claude-code-privacy-proxy · https://github.com/paroque28/claude-code-redact · https://github.com/Open-Source-Lodge/hygienics · https://github.com/JeongJaeSoon/agent-guard · https://github.com/SilentAutomaton/redact-hook · https://github.com/ShindouMihou/cc-redact/ · https://github.com/mintmcp/agent-security |
| **local-llm-mcp** (Sep 2026, v0.1, 0 stars) | A local worker reads real data; the cloud gets summaries with `[EMAIL-1]` placeholders, which are expanded again locally | Everything the agent reads with its own tools, since delegation is optional | Only delegated steps | MIT | https://github.com/lealvona/local-llm-mcp |
| Claude Code sandbox `credentials` mask plus `updatedToolOutput` hooks | Commands see a stand-in value and the proxy swaps in the real one; hooks can rewrite what the model sees | All file content except configured credentials | Only through a hook you write | No | https://code.claude.com/docs/en/sandboxing · https://code.claude.com/docs/en/hooks |
| Cursor Runtime Secrets | `[REDACTED]` in tool results, transcripts and commits | Everything else | Yes, Cloud Agents only | No | https://cursor.com/docs/cloud-agent/security-network |
| Docker Sandboxes credential proxy | The agent only sees a stand-in credential | All file content and output | No | — | https://docs.docker.com/ai/sandboxes/security/credentials/ |
| OpenClaw (13 Sep 2026) | Masks registered secrets in tool results | Everything else | Yes, for registered secrets | Yes | https://github.com/openclaw/openclaw/pull/146596 |
| Copilot content exclusion | GA in the Copilot app and CLI on 2 Sep 2026 | Everything not excluded | Unclear | No | https://github.blog/changelog/2026-09-02-content-exclusions-generally-available-in-copilot-app-and-cli/ |
| Stacklok CodeGate | Prompt-level secret encryption | — | — | Archived 5 Jun 2025 | https://github.com/stacklok/codegate |

Other notes:
- Claude Code stores secrets unredacted in five on-disk stores (open issue, 20 Sep 2026): https://github.com/anthropics/claude-code/issues/95680
- Codex does not strip KEY/SECRET/TOKEN variables from spawned commands by default: https://learn.chatgpt.com/docs/config-file/config-advanced
- Codex deny-globs reportedly do not block reads in v0.130: https://github.com/openai/codex/issues/22179
- Gemini CLI's sandbox is off by default: https://geminicli.com/docs/cli/sandbox/

Mainstream agent sandboxes (Claude Code, Codex, Cursor, Gemini CLI) restrict only what commands can
reach on the filesystem and network. None of them controls what reaches the model.

## 4. Multi-model coding agents: split for cost and capability, never for privacy

- Aider architect/editor: https://aider.chat/docs/usage/modes.html
- Cline plan/act models: https://docs.cline.bot/core-workflows/plan-and-act
- OpenCode per-agent models: https://opencode.ai/v2/docs/agents/
- Crush large/small models: https://deepwiki.com/charmbracelet/crush/4.3-model-configuration
- Continue model roles: https://docs.continue.dev/customize/model-roles/intro
- Goose removed lead/worker and planning mode: https://github.com/aaif-goose/goose/pull/12061
- Roo Code was archived in May 2026.
- Claude Code subagents are Claude-only; routing them to a local model is an open request: https://github.com/anthropics/claude-code/issues/38698
- An August 2026 survey found no privacy boundary in any shipped mixed local/cloud agent: https://dev.to/jacksonxly/running-local-and-cloud-models-in-the-same-coding-agent-what-actually-ships-in-2026-18eo

## 5. Agent-security architectures: same shape, opposite goal

| System | Protects | Trusts the model provider? | Utility cost | Coding? |
|---|---|---|---|---|
| Dual LLM (Willison 2023) | Integrity (injection) | Yes | "Degraded UX" | No |
| CaMeL (DeepMind 2025) | Injection and exfiltration to third parties; a local Q-LLM is mentioned only as a side benefit | Yes | AgentDojo 84% → 77%; ~2.8× tokens | No |
| FIDES (Microsoft 2025) | Integrity and confidentiality labels; quarantined output capped by schema (a boolean is 1 bit) | Yes | Up to +16.7% over the basic planner | No |
| Progent | Least-privilege tool policy | Yes | Attack success 39.9% → 1.0% | Framework only (OpenHands) |
| RTBAS | IFC, integrity and privacy | Yes | −2% | No |

Links:
- Dual LLM: https://simonwillison.net/2023/Apr/25/dual-llm-pattern/
- CaMeL: https://arxiv.org/abs/2503.18813
- FIDES: https://arxiv.org/abs/2505.23643
- Progent: https://arxiv.org/html/2504.11703v3
- RTBAS: https://arxiv.org/abs/2502.08966
- Design patterns: https://arxiv.org/html/2506.08837v2

**Coding-agent exfiltration incidents that shape the threat model:**

| Incident | Channel |
|---|---|
| Claude Code CVE-2025-55284 | DNS via `ping` |
| "Claude Pirate" | The allowlisted Anthropic API with an attacker's key |
| CamoLeak (Copilot, CVSS 9.6) | GitHub's image proxy |
| Cursor CVE-2026-26268 | `.git` hooks |
| "Comment and Control" (Apr 2026) | Tokens from Claude Code, Gemini CLI and Copilot |
| s1ngularity | Malware drove agent CLIs to inventory secrets |

Sources:
- https://embracethered.com/blog/posts/2025/claude-code-exfiltration-via-dns-requests/
- https://embracethered.com/blog/posts/2025/claude-abusing-network-access-and-anthropic-api-for-data-exfiltration/
- https://www.legitsecurity.com/blog/camoleak-critical-github-copilot-vulnerability-leaks-private-source-code
- https://nvd.nist.gov/vuln/detail/CVE-2026-26268
- https://oddguan.com/blog/comment-and-control-prompt-injection-credential-theft-claude-code-gemini-cli-github-copilot/
- https://www.wiz.io/blog/s1ngularity-supply-chain-attack

## 6. How Duet compares

Duet evidence is from `duet_v2/docs/PLAN.md` §10 and the running `results/m52-xl` batch (self-reported, not audited).

**Where Duet is ahead**

1. **Coverage.** It is the only coding agent found that combines all of these:
   - sensitivity by where content comes from (paths and kinds), not only by detection;
   - a local model as the reader (`ask_local`, digests);
   - reversible placeholders;
   - an OS sandbox for every command (credential stores unreadable; no network, or package registries only through the egress proxy; `.git` read-only);
   - one type-level, fail-closed outbound gate;
   - a hash-chained audit log.

   Each add-on in §3 covers one or two of these, and several say they can be bypassed. local-llm-mcp: the agent's own tools bypass it. cc-redact: `@.env` bypasses it. Claude Code's sandbox: Bash only.
2. **Measurement on real coding tasks.** m52-sl, 54 runs: privacy mode scored 98.3% on hidden tests against 97.5% for the frontier alone, with 0 leaks against 3,720 planted canaries (18/18 runs). In the XL batch so far, hybrid runs have 0 leaks while passthrough runs leak 5,300–8,300 canaries each. No paper found measures this end to end on coding; the nearest are PlanTwin (synthetic) and SlotGuard (a repository subset).
3. **The local model earns its place.** SlotGuard shows deterministic substitution alone can reach 0.0% on credentials. Duet's no-local mode is also leak-free but scores 85.3% against 98.3% and costs 2.28× against 1.18×. So the local reader is what buys the quality, not just the privacy.

**Where Duet is behind or shares the field's open problems**

1. **Free-text local answers are an unbounded channel.** SECURITY.md already lists paraphrase as a known gap. Duet's canaries catch exact, spelled-out, base64 and hex copies, not inference. The literature is blunt about this channel:
   - PlanTwin: 94.1% re-identification without disclosure budgets.
   - AgentLeak: inter-agent messages leak 68.8%.
   - LLM-Redactor: 31–43% of code leaks.
   - Hartmann: 53.8% mapping leak.

   FIDES caps quarantined output with a schema, and PlanTwin enforces per-object budgets; Duet has neither.
2. **Quality on the hardest tier.** XL X2: hybrid 63.6% against passthrough 96.4% (seed 1), after 64% against 84–100% in calibration. The misses are formats the frontier sees only as summaries, plus number-precision edge cases. PlanTwin and SlotGuard report ≤2.2–4 points of loss, but on easier or synthetic tasks.
3. **Speed.** Hybrid takes about 2× the wall time on small-to-large tasks (539 s against 259 s) and 2.5–4× on XL. Hooks and proxies add almost no latency.
4. **Integrity.** Duet's boundary is about confidentiality. Its injection defenses (settings never read from files, the sandbox, fixed tools) are not a CaMeL- or Progent-style action policy.
5. **Maturity and distribution.** Duet is unreleased and unaudited. Perplexity, GitGuardian and CrowdStrike ship to large user bases, and Claude Code's `mask` plus hooks let a determined user assemble part of the boundary on the incumbent agent.

## 7. What to take from this

1. **Bound the local-answer channel:** typed or schema-constrained `ask_local` answers where the question allows, plus per-object disclosure budgets (PlanTwin, FIDES).
2. **Add a structural leak test to duet-eval:** a re-identification probe along the lines of PlanTwin's 15-candidate match, next to the canaries. The canaries then cover strings, and the probe covers inference.
3. **Publish the canary and proxy measurement.** Nobody has published one for coding agents, and it is Duet's strongest differentiator (Grok Build's wire analysis shows the demand).
4. **Position against the add-ons as "integrated, fail-closed and measured".** The individual mechanisms (hooks, masks, proxies) are becoming commodities.
5. **Watch Perplexity Hybrid Compute.** It is the first commercial product with Duet's split; a coding surface from Perplexity or a copycat would be direct competition.
