<div align="center">

<img src="site/public/icon.svg" alt="Duet logo" width="88">

# Duet

**Let a cloud model write your code. Keep your secrets on your machine.**

A terminal coding agent where a frontier model does the coding<br>
and your **local model** reads the sensitive files.

[![Latest release](https://img.shields.io/github/v/release/maximpri/duet?include_prereleases&label=release&color=2ea043)](https://github.com/maximpri/duet/releases/latest)
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue)](LICENSE)
[![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey)](docs/INSTALLATION.md)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-dea584?logo=rust)](ARCHITECTURE.md)
[![unsafe forbidden](https://img.shields.io/badge/unsafe-forbidden-success)](Cargo.toml)

[Quickstart](#quickstart) · [Features](#features) · [How it works](#how-it-works) · [Results](#results) · [FAQ](#faq) · [Docs](docs/README.md)

<img src="docs/assets/demo/duet-demo.gif" alt="A real Duet session: the task is typed, the agent reads the code while the customer file stays local, fixes the bug and passes the tests; the Privacy panel then shows the customer file was classified sensitive and a password was replaced with a placeholder before the request went out." width="900">

<sub>A real session on fictional customer data, screen text unaltered: tests go from 1 failing to 4 passing, and checks found none of the 13 planted secrets in the 5 cloud requests. Agent work plays at 6× speed. <a href="docs/evidence/recorder-billing-2026-10-04/README.md">Recording and evidence</a> · <a href="docs/assets/demo/duet-demo.mp4">MP4</a></sub>

</div>

## Why Duet?

Coding agents send whatever they read to the cloud: your `.env`, customer CSVs, production logs, the pricing module you'd rather keep private. Running everything locally avoids that, but local models still can't match frontier models at coding.

Duet splits the job:

- 🧠 **The frontier model** (Claude, GPT, Gemini, GLM, …) plans the work and writes the code.
- 🔒 **Your local model** (Ollama, LM Studio, llama.cpp, …) reads the sensitive files and answers the frontier's questions about them.
- 🛂 **Duet** checks every request before it leaves your machine and logs exactly what was sent.

On nine coding tasks, Duet scored **89%** on hidden tests vs. **97%** for the same frontier model with no protection. Checks found **zero** planted secrets in its 1,124 cloud requests, vs. 35,809 copies without Duet. [See the results →](#results)

> Built by an engineer who has worked at major regulated financial institutions, where "just paste it into the AI" is not an option.

## Quickstart

**1. Install** (macOS and Linux, ARM64 and x86-64, no `sudo` or Rust needed):

```sh
curl -fsSL https://raw.githubusercontent.com/maximpri/duet/main/install.sh | bash
```

**2. Run it** in a new terminal, from your project:

```sh
duet
```

On first run, Duet finds your cloud API key and a running local model server, shows what it will save, and asks once. If something is missing, it prints the exact command to fix it.

> **No local model yet?** Install [Ollama](https://ollama.com) and pull one, for example `ollama pull qwen3:8b`. `duet doctor --online` checks that its context window is large enough.

**3. Describe the task:**

```text
you> the billing export double-counts reactivated customers, fix it
```

<details>
<summary>More ways to install</summary>

- **Disk images and archives:** [latest release](https://github.com/maximpri/duet/releases/latest)
- **From source:** `git clone https://github.com/maximpri/duet && cd duet && tools/install.sh` (Rust 1.90+)
- **Inspect first:** download [`install.sh`](install.sh), read it, run `bash install.sh`
- **Skip the PATH change:** `… | bash -s -- --no-modify-path`

Preview releases are **unsigned**: checksums detect changed files but don't authenticate the publisher. Signature verification, Linux requirements (glibc, bubblewrap) and more: [Installation guide](docs/INSTALLATION.md).

</details>

## Features

- 🔒 **Private by default.** `.env*`, keys, `data/**`, `*.csv`, databases and logs are handled locally out of the box. Built-in detectors replace secrets, personal data and high-entropy strings in everything else with placeholders like `⟨secret:URL_PASSWORD#1⟩`.
- 🧩 **Share code at the level you choose.** Mark a module **interface-only** (the frontier sees signatures, not bodies) or **sealed** (it only knows the file exists).
- 🔍 **See exactly what left your machine.** The **Privacy** panel shows every outbound request and what was filtered. Audit logs are hash-chained, and you can check them with `duet audit verify`.
- 🏠 **Fully local mode.** `duet --mode local-only` hands the whole task to your local model, with no cloud calls, no web tools and no network for commands.
- ✅ **Finish only when tests pass.** `duet --check 'npm test' "…"` keeps the agent working until your checks pass, within its budget.
- 🗺️ **Plan before you change.** `/plan <task>` investigates and saves a plan you can review, edit and approve before anything is written.
- 📦 **Sandboxed commands.** Commands run under Seatbelt (macOS) or bubblewrap (Linux). Network access is limited to package registries.
- 💸 **Budgets.** Per-session dollar limits, with live token and cost tracking.
- 🔌 **Extensible.** MCP servers, language servers (LSP), web search, portable `SKILL.md` skills and plugins, all behind the same privacy checks.

## How it works

<div align="center">
<img src="docs/assets/infographics/duet-boundary-gpt.png" alt="Private files go to local handling, then through Duet's checks; only permitted code and checked context reach the frontier model." width="820">
</div>

1. **The frontier asks for what it needs.** Ordinary code is shared as-is. For a sensitive file, it gets the structure (column names, status values, synthetic rows) and can ask your local model specific questions.
2. **Duet checks what leaves.** Local answers and all other outbound context are filtered and checked against the private values Duet has seen in the run. The local model can't approve its own answers.
3. **The agent edits and tests** in a sandbox. With `--check`, failures go back to the agent until your checks pass.
4. **You review** the patch in **Changes**, the outbound context in **Privacy**, and the saved audit log.

The design is enforced in code: the agent can't construct a model client, and its only path to the frontier goes through the checked gate. [Architecture](ARCHITECTURE.md) · [Security design](docs/SECURE_BY_DESIGN.md) · [Visual guide](docs/DUET_VISUAL_GUIDE.md)

## Results

Nine coding tasks with planted private data (customer records, credentials, logs, proprietary pricing), each run three times per mode with the same frontier model (`glm-5.3-flash`), and scored by hidden tests.

| | Duet | Same model, no protection |
| --- | ---: | ---: |
| Mean hidden-test score | **89.3%** | 96.5% |
| Planted values found in cloud requests | **0** | 35,809 |
| Cloud requests checked | 1,124 | 1,328 |

Duet scored **100% in all three runs on four tasks**. Most of the gap comes from two tasks: billing export and the SQL gateway.

<div align="center">
<img src="docs/assets/infographics/duet-task-results-2026-10-04-gpt.png" alt="Per-task results averaged across three runs: Duet reaches 100% on four tasks; billing export averages 66.67% and SQL gateway 64.67%." width="820">
</div>

**Fine print:** every outcome counts, including one compile failure and one stopped run scored zero. The check looks for literal planted values in recorded requests, so it can't rule out leaks through summaries or inference. [Method and full evidence](docs/evidence/benchmark-54-2026-10-04/README.md) · [Per-task scores](docs/DUET_VISUAL_GUIDE.md#results-by-task) · [Raw JSON](docs/evidence/benchmark-54-2026-10-04/report.json)

## Usage

```sh
duet                                   # interactive session (hybrid mode)
duet "fix the failing export"          # start with a task
duet --check 'cargo test' "fix it"     # done only when the check passes
duet --mode local-only                 # nothing leaves this machine
duet --resume                          # continue the last session
duet run "fix the export"              # one-shot, non-interactive
duet privacy                           # what's sensitive and where requests go (offline)
duet audit show <run>                  # every request sent to the cloud
duet doctor                            # diagnose setup, with a fix for each problem
```

Mark private code in `.duet/config.toml`:

```toml
[sensitivity]
protected_paths = ["src/billing/**"]    # source read only by the local model

[ip]
interface_only = ["src/pricing/**"]     # frontier sees signatures only
sealed = ["src/risk_model/**"]          # frontier knows it exists, nothing more
```

Inside a session: `/plan`, `/privacy`, `/mode top-clearance`, `/help`, and F2 for settings. [Full usage guide](docs/USAGE.md)

## Supported models

| Cloud (frontier) | Local |
| --- | --- |
| Anthropic · OpenAI · Google Gemini · OpenRouter · z.ai GLM · DeepSeek · xAI · Mistral · Groq · Cerebras · Together · Fireworks · Qwen | Ollama · LM Studio · llama.cpp · vLLM · oMLX · MLX · Jan · GPT4All · KoboldCpp · LocalAI · LiteLLM |

Any OpenAI-compatible endpoint works too. Live benchmark runs so far used z.ai `glm-5.3-flash` with Qwen on oMLX. The other backends pass protocol and discovery tests; please [report](https://github.com/maximpri/duet/issues) how they work for you. [Model setup](docs/USAGE.md#models)

## FAQ

<details>
<summary><b>Does Duet guarantee nothing private reaches the cloud?</b></summary>

No. It guarantees that requests pass through checks that filter and block known private values, and it records what was sent so you can verify. A summary from your local model can still reveal facts ("3 customers are overdue"). Read [what is and isn't covered](docs/SECURE_BY_DESIGN.md) before using it with real regulated data.
</details>

<details>
<summary><b>How is this different from Claude Code, Codex or Aider?</b></summary>

With a cloud model, those agents send the files they read to the provider. Duet adds a second, local model that reads the sensitive files, plus an enforced outbound gate with an audit log. You keep a frontier model for the actual coding.
</details>

<details>
<summary><b>What hardware do I need?</b></summary>

Enough to serve a local model with a context window of about 40K tokens. In the default hybrid mode the local model only reads and answers questions, so it doesn't need to be a strong coder. In local-only mode it does all the coding.
</details>

<details>
<summary><b>Can I try it without a local model?</b></summary>

Yes, with protection off: `duet --mode passthrough --no-privacy` sends everything to the frontier unfiltered, like a regular coding agent. Use it to try the workflow on non-sensitive code.
</details>

<details>
<summary><b>Is it production-ready?</b></summary>

It's a development preview: 1,300+ tests, fuzzing, and `unsafe` forbidden across 165K lines of Rust, but no external audit yet and unsigned releases. Feedback and bug reports are very welcome.
</details>

## Contributing

Bug reports, feature ideas and "it didn't work on my setup" reports are the most helpful contributions right now: [open an issue](https://github.com/maximpri/duet/issues). Code contributions open once the CLA is published ([details](CONTRIBUTING.md)). To report a vulnerability, use a [private advisory](https://github.com/maximpri/duet/security/advisories/new) ([policy](SECURITY.md)).

If Duet is useful to you, a ⭐ helps others find it.

## License

[GPL-3.0-or-later](LICENSE). Release archives include the corresponding source and third-party notices ([licensing](LICENSES.md)).
