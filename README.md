# Duet

**Frontier-level coding results, with sensitive information processed only by a local model.**

> Status: **in development, not released.** The `duet` CLI, the security engine, IP levels and the
> evaluation harness work; the privacy and quality gates have passed; cost is reported as a measured privacy premium. Setup,
> `duet doctor` and the TUI come later (M6). Progress and measurements: [docs/PLAN.md](docs/PLAN.md) §10.

## What it is

Duet is a coding agent for repositories that contain things you shouldn't send to a cloud model —
credentials, customer data, production logs, regulated data, or your most valuable source code.

- A **frontier model** plans, decides every step and writes the code, so you get frontier-quality
  work.
- A **local model** on your machine is the only model that ever reads sensitive content. It
  summarizes it, answers questions about it and rewrites protected code on request; it has no tools
  and never decides what to do next.
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
- **Logs and data files** appear as a handle with error lines and values replaced, their repeated
  line shapes and a local summary. The frontier asks the local model questions about them
  (`ask_local`), but never sees the raw content. At the start of a run the local model also briefs
  the frontier on what the sensitive files show for the task, with values withheld.
- **Commands cannot read sensitive files** (OS sandbox). A command that must, such as the program
  run on the real data, is marked `sensitive_data`: its output stays local behind a handle and the
  files it writes become sensitive.
- **Large public results** (long files, outputs, listings, searches) appear as the first lines and
  an outline; the frontier reads the ranges it needs.
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
| Frontier-level results | On a suite of tasks from 15-minute fixes to multi-hour changes, code quality (blind judge) within 2/30 of the same frontier model alone, and hidden-test pass rate not behind it on most tasks |
| Sensitive data stays local | Zero planted canaries in outbound traffic, measured by an independent logging proxy; every request is also in Duet's audit log |
| Cheaper | Lower total cost than the same frontier model alone (API list prices incl. caching, plus local electricity) |
| Reliable | Every run ends as completed, failed with a reason, or budget-stopped; interrupted runs resume |

So far (frontier `glm-5.3-flash`): quality matched the frontier alone, and no planted canary left
in 18 of 18 hybrid runs, while the frontier alone sent canaries in all 18. Duet is not cheaper than the frontier alone: privacy costs
extra frontier turns, because the frontier must ask about data it cannot read, and offloading
bulky content to the local model saves less than those turns cost. Duet therefore states a measured
privacy premium (about 1.4× the frontier alone on the measured tasks) rather than claiming savings. Full results will be published in `docs/BENCHMARK.md` (milestone M5),
with raw data and the method needed to reproduce them.

## Usage

```sh
duet run "fix the failing billing export"      # hybrid mode (the default)
duet audit show <run>           # see exactly what was sent to the frontier, and the security events
duet audit verify <run>         # check the hash chain and its anchor
duet resume <run>               # continue an interrupted run
duet config list                # every setting, its value and where it came from
duet config set --project ip.interface_only '["src/pricing/**"]'
duet local-eval                 # measure the configured local model in its reading roles
duet purge                      # delete raw run data older than the retention period
```

`duet run --mode passthrough --no-privacy` runs the frontier alone with the boundary off (the
evaluation baseline). Planned for M6: `duet setup`, `duet doctor` and `duet tui`.

### Models

- **Frontier:** z.ai `glm-5.3-flash` by default; any OpenAI-compatible Chat Completions endpoint.
  Responses and Anthropic Messages endpoints are planned (M6).
- **Local:** any OpenAI-compatible Chat Completions server (oMLX, LM Studio, llama.cpp, vLLM,
  Ollama) on loopback, or on a host the owner allowlists; plain HTTP to a non-loopback host needs
  `local.allow_plaintext`. The default, `omlx-coding` (Qwen 3.8 27B on oMLX), was chosen with
  `duet local-eval`.

### Configuration

Every setting is defined in one registry and editable with `duet config` (and the TUI, in M6):
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
| [SECURITY.md](SECURITY.md) | Threat model, secure defaults, known limits, advisories, reporting a vulnerability |

## Building

Rust only.

```sh
cargo build --release
tools/gate.sh        # formatting, lints, tests, license and provenance checks
```

## License

GPL-3.0-or-later.
