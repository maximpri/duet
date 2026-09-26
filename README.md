# Duet Core

**Frontier-level coding, with your sensitive information read only by a model on your own hardware.**

> Status: **in development, not released.** Everything described here is built and tested, some
> parts only against simulated servers; the measurements below are from Duet's own benchmark.
> There is no independent audit or certification yet. Progress: [docs/PLAN.md](docs/PLAN.md) §10. What is verified and what is not:
> [docs/ACCEPTANCE.md](docs/ACCEPTANCE.md).

## The problem

AI coding agents read everything in a repository to do their work: source code, `.env` files,
configuration, test fixtures, logs, database dumps, customer exports. Whatever they read, they send
to the cloud model in plain text. For many companies that rules them out:

- **Security**: credentials, keys and connection strings leave the building every time an agent
  opens a config file or prints the environment.
- **Privacy and regulation**: customer records, card numbers, national IDs and account numbers in
  fixtures and logs become personal data processed by a third party, possibly in another
  country, which brings banking secrecy, PCI DSS and GDPR transfer questions.
- **Intellectual property**: the pricing engine, the trading model or the algorithm that is the
  company's edge ends up on someone else's servers.

The usual answers each give something up:

| Approach | Where you end up |
|---|---|
| A frontier agent under an enterprise contract | The provider still receives everything in plain text. "No training" and "no retention" are promises you cannot verify, and abuse monitoring, legal holds and the retention some top models require keep data anyway |
| A redaction proxy in front of a cloud agent | Detectors miss what they don't recognize, the agent's own tools (terminal, file reads, plugins) route around exclusion lists, and masking everything degrades the answers |
| A local or air-gapped model only | Nothing leaves, but you lose frontier-level quality exactly where agents need it: planning, multi-step changes, debugging |

## The premise

**The model that decides what to do does not need to read your secrets.**

Most of the work in a coding task needs the *structure* of sensitive content, not its values:

- To fix a billing export, the model needs to know the CSV has seven columns, dates in two formats,
  and three rows with an empty amount. It doesn't need the customers' names.
- To wire up a database, it needs to know that a `DATABASE_URL` exists and where it is used. It
  doesn't need the password.
- To change code that calls a proprietary pricing engine, it needs the engine's signatures and
  types. It doesn't need its formulas.

Duet splits the work along that line:

- A **frontier model** in the cloud plans, decides every step and writes the code. You get
  frontier-level results.
- A **local model** on your hardware is the only model that ever reads sensitive content. It
  summarizes it, answers questions about it and implements changes to protected code. It has no
  tools and never decides what happens next.
- **The machine enforces the boundary between them, not a policy document.** An OS sandbox, one
  outbound gate and a tamper-evident audit log make it hold, and planted canaries measure whether it
  does.

```
                          ┌──────────────────────────── your machine ────────────────────────────┐
                          │                                                                      │
 frontier model  ◄──────  │  outbound gate  ◄──  what the frontier may see:                      │
 (cloud: plans,           │  (checks every byte,  public code, placeholders ⟨secret:DB_URL#1⟩,   │
  decides, writes code)   │   fail-closed,        handles, local summaries and answers           │
                          │   audit log)                        ▲                                │
                          │                                     │                                │
                          │  your repository ──► security engine ──► local model                 │
                          │                      secrets, personal     (reads, summarizes,        │
                          │                      data, logs, data      answers; no tools)         │
                          │                      files, protected                                 │
                          │                      code                                             │
                          └──────────────────────────────────────────────────────────────────────┘
```

## How Duet keeps the premise

### 1. Sensitive by default, classified by where content comes from

Secrets and `.env*` files, personal data, data files and databases, logs, and the output of any
command that touches them are sensitive unless you say otherwise. Detection covers 222 maintained
secret-format rules (the gitleaks rule set, imported as data), Duet's own secret detectors, and
personal data in US, UK and EU formats: card numbers, IBANs, national IDs with their check digits,
international phone numbers and addresses. Your own patterns can be added. Anything labelled as a
card or account number is protected even if its checksum fails, and so is any long number you type.

### 2. The frontier works with stand-ins, never the values

| What is in your repository | What the frontier receives |
|---|---|
| `DATABASE_URL=postgres://app:s3cr3t@db.internal/prod` | `DATABASE_URL=⟨secret:DATABASE_URL#1⟩`. When it writes code or config with the placeholder, Duet fills in the real value locally, and only where a secret belongs |
| `data/customers.csv` (40,000 rows) | A handle, the file's structure (columns, types, value shapes such as `9999-99-99` or `Aa Aa`, null and empty counts, anomalies; never a value), a synthetic record with the same formats, and a summary written by the local model. `synthetic_sample` gives more fake records to use as test fixtures; it asks the local model questions (`ask_local`) instead of reading rows |
| `logs/prod.log` | A handle, its line templates (values and unknown words as shapes) and a local summary |
| A card number you type in the task | `⟨card:card#1⟩`, which the frontier can ask the local model about. Your own screen shows the real value |
| `src/pricing/engine.rs` marked **interface-only** | Signatures, types and doc comments; function bodies withheld |
| `src/pricing/model.rs` marked **sealed** | Only that it exists |

To change protected code, the frontier describes the change and writes the tests. The local model
implements it where the code can be read, and Duet runs the tests. The frontier learns only whether
they pass.

### 3. The local model reads, and what it writes is filtered too

The local model is the component that reads hostile or sensitive text, so it is given no tools and
its output is treated as data:

- **Copied text is cut.** Runs of four or more words copied from sensitive content are removed from
  local answers, and so are fragments of four or more digits of a protected number.
- **Disguised values are caught.** Values spelled out, split up or base64/hex-encoded are matched
  against what the vault holds.
- **Probing is limited.** Questions that try to extract a value piece by piece ("what are the first
  four digits?") get an answer about the value's format instead. Each value has a small budget of
  characters for the whole run, and every probe is logged.

### 4. The operating system enforces the boundary

- **Commands cannot read sensitive files.** Every command the agent runs is sandboxed (Seatbelt on
  macOS, bubblewrap with seccomp on Linux). Sensitive paths and `.git` are unreadable to ordinary
  commands, no command can read Duet's own state or the credential stores in your home directory
  (`~/.npmrc`, `~/.ssh`, `~/.aws`, shell histories, ...).
- **Commands reach package registries, and nothing else.** By default a command's only network is
  Duet's egress proxy, which lets `npm install`, `cargo add` or `pip install` through to the
  registries you allow and refuses every other host, private addresses and mismatched TLS names;
  each connection is in the audit log. What a command could send there is what it can read, which
  excludes everything sensitive. A command marked `sensitive_data` has no network at all.
- **Sensitive commands keep their output local.** A command that genuinely needs the real data,
  such as running the program on a production sample, is marked `sensitive_data`. Its output stays
  local behind a handle (shown with every value masked by its shape), and every file it writes
  becomes sensitive too, so derived data can't leak through a second, ordinary read. A short
  output (a count, a match, a yes or no) is a probe of the data: a run shows a dozen, then
  withholds them.

### 5. One gate, fail-closed, with evidence

Every request to the frontier passes one outbound gate:

- **It is filtered and checked.** Known values and copied sensitive text are replaced in every
  message, including the model's own earlier text and tool arguments. A final check, which reads
  the request exactly as the filter does, refuses to send if any known value remains; a part it
  still refuses is withheld from the request (and the audit log says so), and only a request that
  cannot be cleared that way stops the run.
- **It is recorded.** Each request is written to a hash-chained audit log whose head is anchored
  outside the repository.
- **You can inspect it:**
  - `duet audit show` shows exactly what was sent;
  - `duet audit verify` detects any edit or rewrite of the log;
  - `duet audit disclosure` reports what was withheld, by class.

### 6. Every tool follows the same rules

A practical agent needs more than file reads, and each new channel is held to the same contract:
results reach the frontier only through the boundary, outbound text is checked, spawned processes
are sandboxed, every use is audited, and side effects need approval.

| Capability | How the premise holds |
|---|---|
| Sessions (`duet chat`) and steering | Your messages are sanitized like the task, and what you see shows your real values, also as replies stream in; nothing on your screen is stored anew or sent |
| Web search and fetch | Search works without setup and Duet runs it itself: it asks public sources with open APIs (Stack Overflow, Wikipedia, GitHub, your languages' package registries) directly, with no search provider in between; each source asked sees the query, and `duet doctor` lists them. Your own SearXNG, Brave or Z.ai's search only when you choose one. Queries and URLs with a protected value are refused before anything is sent; internal network addresses are unreachable |
| MCP servers (plugins) | Sandboxed. A server you mark sensitive (your database) returns results that stay local behind handles; public servers never receive protected values |
| Language servers | Answers are shown the same way as the file they come from; sensitive files are never opened for them |
| Git history and commits | A file that is sensitive today stays protected in every past revision, and old commits are scanned for secrets. Commits include only files Duet wrote, never sensitive ones, and in a conversation each one waits for your yes |
| Project instructions (`DUET.md`) | Scanned like any repository file before the frontier reads them; they guide how Duet works in a repository but can never change its settings, policy or sandbox |
| Sub-agents | They share the same engine, vault and audit chain, and get no more than their parent |
| Test and build output | Shown condensed (failures, errors with locations, summaries; passing tests and progress left out, the whole output a `read_raw` away), and only when it may be shown at all: it is sanitized whole first, and output held as sensitive stays held |
| Package installs | Commands reach only the registries in `sandbox.registries` (crates.io, npm, PyPI, Go, Maven Central, RubyGems, GitHub downloads) through an audited proxy; `sandbox.network = "off"` takes even that away, `"all"` opens everything (with `--confirm`) |
| Images | Images can't be text-scanned, so the local model describes them unless you mark one public |
| Context compaction (off by default) | The local model condenses a long conversation from what the frontier was already sent; its summary is filtered like any local output and passes the same gate |
| Local explorer (`explore`, off by default) | The local model answers "where/what/how" questions about the code by reading the repository itself, read-only, and the frontier gets one checked report instead of many reading turns; the report is filtered like any local output, and sensitive or protected files appear only as file:line references |

### 7. Secure by default, and the owner decides

- **Repositories cannot loosen policy.** A repository's own configuration can only make policy
  stricter; endpoints, credentials and loosening are reserved to your user configuration.
- **Loosening is deliberate and recorded.** Relaxing a privacy setting needs `--confirm` and is
  audited. Running without the boundary needs an explicit `--no-privacy`, and a repository can
  forbid it altogether (`duet config set --project frontier.allow_passthrough false`).
- **Risky actions can need approval.** An optional approval mode asks before them.

## Measured, not promised

Duet is evaluated against the same frontier model working alone, on the same tasks, with synthetic
secrets and personal data ("canaries") planted in each repository. An independent logging proxy, as
well as Duet's own audit log, records everything sent to the provider.

| Question | Result so far (frontier `glm-5.3-flash`, local Qwen 3.8 27B) |
|---|---|
| Does sensitive data leave? | **0 canaries** in 18 of 18 Duet runs in the latest quality gate. The same frontier model alone sent **3,133 canaries** on the same tasks, in every run |
| Does protected code leave? | **0** protected-source canaries in the IP gate, against 102 hits for the frontier alone |
| Is the work as good? | Yes, within the pre-set margin. Two blind judges from different model families scored 20.1 vs 20.2 out of 30, and hidden tests passed 96.7% vs 95.9% |
| What does it cost? | Frontier spend about **1.45–1.5×** the frontier alone on small-to-large tasks (1.6–1.8× including an upper-bound estimate of the local model's electricity), and about **0.87×** on the largest tasks; the ratios hold when the same tokens are priced at flagship models. It is slower. Privacy costs extra frontier turns, because the frontier has to ask about data it cannot read |

How Duet looks for its own weaknesses:

- **An adversarial scenario suite runs on every commit.** A scripted frontier tries to extract
  values, and a deliberately careless local model echoes, spells out and encodes what it reads.
- **A leak checker looks for disguised forms**: escaped, encoded, reversed and split values, and
  fragments of numbers.
- **The detection rules are tested.** A detection corpus checks an example for every imported
  rule (228 of 228 caught) and every personal-data format, and tracks the false-positive rate.
- **Every leak found is published.** Twenty-one advisories (DUET-2026-001 to 021) each have a
  root cause and a regression test.

Several of those leaks were found by exactly this process, including one in a quality-gate batch
that led to a fix and a full re-run. Full results with method and raw data will be published in
`docs/BENCHMARK.md`.

## What this can mean for compliance

Duet changes *what the frontier vendor receives*: placeholders and summaries instead of card
numbers, credentials and customer records, and interfaces instead of protected code. That can help
with:

- **Minimizing data at the source.** Values are pseudonymized locally, with the mapping never
  leaving your machine, before anything is sent.
- **Narrowing the vendor's scope.** The provider may stay out of scope for data it never receives,
  such as card data or information under banking secrecy or data-residency rules.
- **Evidence for monitoring duties.** A customer-held, tamper-evident record shows what each vendor
  received.

This is an engineering control, not legal advice or a certification. Your contracts, registers and
assessments for the frontier provider still apply. Whether a given regulator accepts local
pseudonymization for a given data flow is a question for your counsel.

## What Duet does not protect

Being precise about limits is part of the design. In short (the full list is in
[SECURITY.md](SECURITY.md)):

- **Open source code is sent to the frontier.** That is how it does frontier-quality work; mark
  paths interface-only or sealed to withhold them. Your task description is sent too.
- **The frontier learns that sensitive content exists and what shape it has**: a key named
  `DB_URL`, a file's columns, how many errors a log holds.
- **Filters recognize forms, not meaning.** A value no detector knows, written in a form the filters
  don't match, or information paraphrased or leaked one yes/no answer at a time, can get through.
  These residual risks are documented and measured.
- **Your own machine and the local model host are trusted.** Duet does not defend against a
  compromised machine or a malicious operator.

## Who it is for

- **Regulated teams** whose repositories, fixtures and logs contain customer, card, health or
  account data, and who today keep them away from AI agents entirely.
- **Companies whose source code is their product or their edge**, and who want frontier help around
  it without exposing it.
- **Security teams** who need evidence, not assurances, of what an AI tool sent where.

## Getting started

You need a frontier API key (Z.ai by default; Anthropic and OpenAI presets exist but haven't
been used live yet) and a local
model served by any OpenAI-compatible server on your machine or a host you allowlist: oMLX, LM
Studio, llama.cpp, vLLM or Ollama. Without a local model, `duet config set local.enabled false`
keeps privacy mode on: the frontier then sees sensitive content only as handles and cannot ask
about it.

```sh
cargo build --release             # the binary is target/release/duet
duet doctor                       # checks models, sandbox, git and audit setup, with a fix for each
duet chat                         # code in a conversation (privacy on by default)
duet run "fix the failing billing export"   # one task, run to completion
duet audit show <run>             # exactly what was sent to the frontier
```

Duet works in any folder, a git repository or not. Put how to work in a repository (conventions,
build and test commands) in `DUET.md` at its root, and it is given to Duet at the start of every
run and session.

On a terminal `duet chat` streams duet's replies as they are written, keeps an editable input line
with your earlier messages of the session, and shows a status line while it works; `duet run`
shows its progress on standard error (`--quiet` for none).

Every command, the tools and all settings: [docs/USAGE.md](docs/USAGE.md).

## Documentation

| Document | Contents |
|---|---|
| [docs/USAGE.md](docs/USAGE.md) | Commands, sessions, tools, models and configuration |
| [SECURITY.md](SECURITY.md) | Threat model, enforcement, detection, known limits, advisories, reporting a vulnerability |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Crates, types, turn lifecycle, boundary internals, state on disk, invariants |
| [docs/TARGET_STATE.md](docs/TARGET_STATE.md) | What Duet is when finished: north star, security engine, IP levels, configuration |
| [docs/PLAN.md](docs/PLAN.md) | Milestones, gates, evaluation method, risks, verification, progress |
| [docs/DOGFOOD_SUITE.md](docs/DOGFOOD_SUITE.md) | Benchmark tasks by complexity tier, canaries, metrics, statistics |

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

There is no hosted CI: `tools/gate.sh` is the gate, and every commit must pass it. Nothing else
enforces that, so the first step after cloning is to install the hooks that run it:

```sh
tools/install-hooks.sh --pre-commit   # pre-push: full gate; pre-commit: tools/gate.sh --fast
```

Then, day to day:

```sh
tools/gate.sh --fast                  # format, license, privacy and provenance checks (seconds)
PROPTEST_CASES=5000 cargo test -p duet-boundary --test no_canary   # a deeper property run
tools/fuzz.sh 60                      # each fuzz target for 60 s (see tools/fuzz.sh --help)
cargo test -p duet-boundary --test corpus -- --nocapture   # detection recall and false positives
tools/update-rules.sh v8.30.1         # check a gitleaks release against the vendored rules (--apply to take it)
```

The hooks are not installed automatically (a clone never runs code on its own). They live in the
repository's hooks directory, so every worktree of a clone shares them;
`tools/install-hooks.sh --uninstall` removes them and `--no-verify` skips them once. Property tests
run in the gate with small case counts; `PROPTEST_CASES` raises them. Fuzzing is not part of the
gate (it needs time, and cargo-fuzz needs a nightly toolchain; without one `tools/fuzz.sh` builds
the targets on stable).

## Editions

**Duet Core** (this repository) is open source under GPL-3.0-or-later: the agent, the security
engine, the sandbox, the outbound gate and the audit log — everything described above.

**Duet Enterprise** is a separate commercial edition for organisations that need to *mandate* and
*prove* how AI coding handles restricted data: an organisation-signed policy enforced on every
developer machine (developers cannot loosen it), a signed receipt per session showing what each
outside party received, an independent verifier, SIEM/GRC export, fleet management and compliance
reports. It is planned, not yet built. It builds on Duet Core's documented embedding API (a
policy layer above the configuration, audit-event subscribers and a run-end hook, and
`duet_cli::main_with`, which runs Duet's command line with them; see
[ARCHITECTURE.md](ARCHITECTURE.md) §13), which Core's own command line does not use.

## License

Duet Core is licensed under the GNU General Public License v3.0 or later ([LICENSE](LICENSE)).

The copyright holder also licenses Duet Core under separate commercial terms for Duet Enterprise
(dual licensing). To keep that possible, contributions are accepted only under a contributor
licence agreement; see [CONTRIBUTING.md](CONTRIBUTING.md).
