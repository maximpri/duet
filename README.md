# Duet

**Frontier-level coding results, with sensitive information processed only by a local model.**

> Status: **design stage.** The architecture and plan are approved; implementation has not started.
> Nothing below is usable yet. See [docs/PLAN.md](docs/PLAN.md) for progress.

## What it is

Duet is a coding agent for repositories that contain things you shouldn't send to a cloud model —
credentials, customer data, production logs, regulated data, or your most valuable source code.

- A **frontier model** plans, decides every step and writes the code, so you get frontier-quality
  work.
- A **local model** on your machine is the only model that ever reads sensitive content. It
  summarizes and answers questions about it; it never acts.
- A **single outbound gate** checks every byte before it leaves your machine and records it in a
  tamper-evident audit log you can inspect.

Today you usually choose between an agent that sends everything to the cloud and a local-only agent
with lower quality. Duet is built so you don't have to.

## How it works

```
frontier model ◄── outbound gate ◄── what the frontier is allowed to see
                     (scan, redact,        │
                      audit log)           │  raw code, placeholders, summaries, answers
                                           │
your repository ──► security engine ──────┘
                        │  secrets, personal data, logs, data files, protected code
                        ▼
                   local model (on your machine): reads, summarizes, answers
```

- **Secrets** appear to the frontier as placeholders like `⟨secret:DB_URL#1⟩`. When it writes code
  or config using a placeholder, Duet fills in the real value locally, and only in places where a
  secret belongs.
- **Logs, data files and command output** appear as short summaries with a handle. The frontier can
  ask the local model questions about them (`ask_local`), but never sees the raw content.
- **Protected source code** can be shown as interfaces only (signatures, types, docs) or hidden
  entirely. Edits to protected code are implemented locally against tests the frontier writes.
- **Everything that leaves** is logged in a hash-chained audit log, together with the security
  decisions of the run, and its head is anchored outside the repository: `duet audit show <run>`,
  `duet audit verify <run>`.
- **Secure by default:** loosening a privacy setting needs `--confirm` and is recorded; the
  boundary-off passthrough mode needs `--no-privacy`; a LAN model over plain HTTP is refused unless
  the owner opts in.

Duet withholds sensitive content; it does not obfuscate it. What the frontier can still infer —
that a file exists, its shape, the intent of your task — is documented in `SECURITY.md`.

## Goals Duet is measured against

| Goal | Measure |
|---|---|
| Frontier-level results | On a suite of tasks from 15-minute fixes to multi-hour changes, passes as many hidden tests as the same frontier model alone (within 5 percentage points) |
| Sensitive data stays local | Zero planted canaries in outbound traffic, checked by Duet's audit log and an independent proxy |
| Cheaper | Lower total cost than the frontier model alone (API prices incl. caching, plus local electricity) |
| Reliable | Every run ends as completed, failed with a reason, or budget-stopped; interrupted runs resume |

Results will be published in `docs/BENCHMARK.md`, with raw data and the method needed to reproduce
them.

## Planned usage

```sh
duet setup                      # choose frontier model and detect local model servers
duet run "fix the failing billing export"
duet audit show <run>           # see exactly what was sent to the frontier
duet resume <run>               # continue an interrupted run
duet config set sensitivity.globs+ "reports/**"
duet tui                        # configure everything interactively; watch runs live
```

### Models

- **Frontier:** z.ai GLM by default; any OpenAI-compatible, Responses or Anthropic endpoint.
- **Local:** Ollama, LM Studio, llama.cpp, vLLM or oMLX, running on your machine (loopback only).

### Configuration

Every setting is defined in one registry and editable from the TUI or `duet config`:
models, sensitivity rules and detectors, protected paths and IP levels, budgets, retention.
Credentials and endpoints live only in your user config (`~/.config/duet/config.toml`); a
repository's `.duet/config.toml` can make privacy stricter but never looser.

## Documentation

| Document | Contents |
|---|---|
| [docs/TARGET_STATE.md](docs/TARGET_STATE.md) | What Duet is when finished: north star, security engine, IP levels, configuration |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Crates, types, turn lifecycle, boundary internals, state on disk, invariants |
| [docs/PLAN.md](docs/PLAN.md) | Milestones, gates, evaluation method, risks, verification |
| [docs/DOGFOOD_SUITE.md](docs/DOGFOOD_SUITE.md) | Test tasks by complexity tier, canaries, metrics, statistics |
| `SECURITY.md` | Threat model (written in milestone M0) |

## Building

Rust only. Not yet buildable. Once scaffolded:

```sh
cargo build --release
tools/gate.sh        # formatting, lints, tests, license and provenance checks
```

## License

GPL-3.0-or-later.
