# Duet

**Frontier-level coding results, with sensitive information processed only by a local model.**

> Status: **in development, not released.** The `duet` CLI, the security engine, IP levels and the
> evaluation harness work; the privacy and quality gates have passed; cost is reported as a measured privacy premium. Local
> backend presets, a no-config bootstrap, `duet doctor` and the `duet tui` terminal UI exist. Progress and measurements: [docs/PLAN.md](docs/PLAN.md) §10.

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
duet audit disclosure <run>     # what was withheld from the frontier, by class (counts only)
duet resume <run>               # continue an interrupted run
duet config list                # every setting, its value and where it came from
duet config set --project ip.interface_only '["src/pricing/**"]'
duet config preset              # local backends: default ports, where they report models and context
duet config preset ollama --model qwen3:8b --confirm   # point the local role at one (audited)
duet doctor                     # pass/warn/fail with a fix per check; no network (--online, --json)
duet tui                        # settings, IP levels, audit viewer and run view in the terminal
duet local-eval                 # measure the configured local model in its reading roles
duet purge                      # delete raw run data older than the retention period
```

`duet run --mode passthrough --no-privacy` runs the frontier alone with the boundary off (the
evaluation baseline).

**`duet tui`** has seven screens: Models (frontier and local settings, with `duet doctor`
offline; `o` adds the `--online` checks), Sensitivity (globs, detectors, raw-output commands,
secret sinks, bulky thresholds, and a tester that shows whether a path is sensitive and which
pattern matched), IP levels (the workspace tree as git sees it, so `.gitignore` applies; `i` / `s`
mark a file or directory interface-only / sealed, with a preview of the skeleton the frontier would
see), Limits, Data, Audit (each run's records and outbound request summaries as stored, and `v` to
verify the hash chain and its anchor) and Run (follows a run's turns, tool calls and withheld
content as they are written; read-only). The settings screens are generated from the registry and
show where each value comes from (default, owner or project). Edits take the same path as
`duet config set`: a change that loosens privacy shows its diff and needs `y`, `p` switches edits
to the project file (which only tightens and never takes owner-only keys), and every applied change
is recorded in the owner's config audit log.

**Operator approval** (`oversight.approve`, owner config only; default `off`): with `risky`, Duet
asks y/N on the terminal before a `sensitive_data` command, a protected edit, or a write to anything
other than an ordinary source or test file; with `all`, before every command and write. A refusal
is returned to the model as a tool error, and every decision is in the run's audit log. A run with
approval on and no terminal refuses to start. Details: [SECURITY.md](SECURITY.md) (Oversight).

**Getting a local model.** With no `local.base_url` in your user config, `duet run` looks for a
server on this machine only (127.0.0.1 on the preset ports 11434, 1234, 8080 and 8000, or
`DUET_LOCAL_PORTS`), lists what answered, and uses it for that run when exactly one model is on
offer (saying so, and recording it in the run). With several, or none, it prints the exact
`duet config set` commands and stops. It never writes configuration: `duet config preset <name>`
does that, through the same `--confirm` and audit path as any endpoint change.

**`duet doctor`** checks the configuration and its origins, the config audit chain, settings looser
than their defaults, the frontier endpoint and whether its key variable is set (the value is never
printed), local-endpoint trust (loopback, allowlist, the plain-HTTP rule), the sandbox, git, disk
space, the audit chains and anchors of the latest runs, run data past retention, the approval
mode, and whether release signing keys are present (a warning in a build made by
`tools/release.sh`; a note in a development build, since no release has been published). It uses no
network by default. `--online` adds one model listing per configured server (and the local model's
context window); it never calls a model. Exit code: 0 pass, 1 warn, 2 fail.

Live smoke tests, one per local backend, run with
`DUET_LIVE_OLLAMA_URL=http://127.0.0.1:11434/v1 cargo test -p duet-boundary --test backend_smoke -- --ignored`
(also `LMSTUDIO`, `LLAMACPP`, `VLLM`, `OMLX`, `MLX`; `DUET_LIVE_<BACKEND>_MODEL` picks the model).

### Models

- **Frontier:** z.ai `glm-5.3-flash` by default; any OpenAI-compatible Chat Completions endpoint.
  Responses and Anthropic Messages endpoints are planned (M6).
- **Local:** any OpenAI-compatible Chat Completions server (oMLX, LM Studio, llama.cpp, vLLM,
  Ollama) on loopback, or on a host the owner allowlists; plain HTTP to a non-loopback host needs
  `local.allow_plaintext`. The default, `omlx-coding` (Qwen 3.8 27B on oMLX), was chosen with
  `duet local-eval`.

### Configuration

Every setting is defined in one registry and editable with `duet config` or `duet tui`:
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

A release is signed and comes with an SBOM. The operator provides an SSH signing key (never stored in
the repository):

```sh
tools/release.sh 0.1.0 --key ~/.ssh/duet_release   # gate, --locked release build, SBOM, SHA256SUMS + signature
tools/verify-release.sh dist/duet-0.1.0            # signature (allowed_signers) and every checksum
tools/sbom.sh --out duet.cdx.json                  # CycloneDX SBOM alone (offline, no plugin)
```

To verify a release, put the published signer line in `~/.config/duet/allowed_signers`. Details:
[SECURITY.md](SECURITY.md) (Supply chain).

## Development

There is no hosted CI: `tools/gate.sh` is the gate, and every commit must pass it.

```sh
tools/install-hooks.sh --pre-commit   # pre-push: full gate; pre-commit: tools/gate.sh --fast
tools/gate.sh --fast                  # format, license, privacy and provenance checks (seconds)
PROPTEST_CASES=5000 cargo test -p duet-boundary --test no_canary   # a deeper property run
tools/fuzz.sh 60                      # each fuzz target for 60 s (see tools/fuzz.sh --help)
```

The hooks are not installed automatically; `tools/install-hooks.sh --uninstall` removes them and
`--no-verify` skips them once. Property tests run in the gate with small case counts;
`PROPTEST_CASES` raises them. Fuzzing is not part of the gate (it needs time, and cargo-fuzz needs a
nightly toolchain; without one `tools/fuzz.sh` builds the targets on stable).

## License

GPL-3.0-or-later.
