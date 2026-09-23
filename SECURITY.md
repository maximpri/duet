# Security and Threat Model

Duet's security claim is narrow and testable: **content Duet classifies as sensitive or protected
is not disclosed to the cloud frontier model provider.** This document states what that covers,
what it does not, and how the claim is verified. Design details: [ARCHITECTURE.md](ARCHITECTURE.md)
§5 and [docs/TARGET_STATE.md](docs/TARGET_STATE.md) §6.

## What is protected

| Asset | Default classification | What the frontier receives instead |
|---|---|---|
| Secrets and credentials (`.env*`, keys, tokens, connection strings, secrets detected in any file) | Sensitive | Placeholders such as `⟨secret:DB_URL#1⟩` |
| Personal data (email, phone, card, national IDs, IBAN, IP, names in data files) | Sensitive | Placeholders, or a handle with a local summary |
| Data files and databases (`data/**`, `*.csv`, `*.db`, `*.sqlite`, `*.parquet`) | Sensitive | Handle + local summary; answers via `ask_local` |
| Logs (`logs/**`, `*.log`) | Sensitive | Handle + local summary |
| Output of commands that read sensitive files, and files those commands write | Sensitive | Handle + local summary |
| Other command output | Scanned | Shown with detected and known values replaced (large output: handle + summary) |
| Large public results (files, allowlisted command output, searches, listings) | Public | Handle + first lines and outline; ranges on request (`read_raw`), scanned like any public content |
| Git history (`git log`, `git show`, `git diff`) | Sensitive | Handle + local summary; a public-only `diff` tool |
| Source code marked Interface-only | Protected | Signatures, types and doc comments; bodies withheld |
| Source code marked Sealed | Protected | Existence only |

## What is not protected

- **Source code left Open** (the default for code). It is sent to the frontier so it can do
  frontier-quality work. Mark paths Interface-only or Sealed to withhold them.
- **The task description** you give Duet, and the architecture visible in skeletons.
- **Existence and shape of sensitive content**: that a key named `DB_URL` exists, a file's column
  count, how many errors a log contains.
- **Paraphrase**: short local answers can convey meaning that no filter recognizes as copied text.
  Answers are length-capped, schema-bound and logged.
- **The local machine, the local model server and the operator.** Duet does not defend against a
  compromised machine, a malicious local model host, or the person running it.

## Trust assumptions

- The owner's configuration (`~/.config/duet/config.toml`) is trusted. Repository content,
  including a project's `.duet/config.toml`, is **untrusted**: it can make policy stricter but can
  never loosen it or set credentials, endpoints or the local model address.
- The local model runs on loopback, or on a LAN host the owner explicitly allowlists. For a LAN host,
  the network path is part of the trusted base; use an SSH tunnel or TLS so sensitive content does
  not cross the network in clear text. `duet doctor` warns about unencrypted non-loopback local
  endpoints.
- The frontier provider is treated as an honest-but-curious recipient: everything it receives may be
  retained.

## Enforcement

1. **Classification** of every tool result by independent layers (path, source, secret and PII
   detectors, local assist, taint, IP marks). Any layer can mark content sensitive; none can unmark
   it.
2. **Transformation** into placeholders, handles and summaries before content enters the
   frontier's context.
   **Command access control**: sensitive paths are unreadable to commands, enforced by the OS
   sandbox (Seatbelt / bubblewrap), so no program can print them in any encoding. A command that
   must read them is run with `sensitive_data`; its output is then held locally like a data file, and
   every file it creates or changes is treated as sensitive from then on.
   **Protected source** (`ip.interface_only`, `ip.sealed`): see the next section.
3. **One outbound gate**, the only code path to the frontier: known values are re-tokenized, the
   payload is re-scanned, copied spans of sensitive content (≈24+ tokens) are removed, and nothing
   is sent while any check fails.
4. **Hash-chained audit log** of every outbound request (placeholder-substituted), verifiable with
   `duet audit verify <run>`.
5. **Write-back rules**: secrets are resolved locally and only into places where secrets belong.
6. **Local data hygiene**: raw handles, transcripts and the placeholder vault are mode 0600 under
   `.duet/runs/`, deleted after the retention period or by `duet purge`.

## Protected source (IP levels)

Source files matched by `ip.interface_only` are shown to the frontier as a skeleton: signatures,
type definitions, doc comments and public constants, parsed with tree-sitter (Rust, TypeScript,
Python). Function bodies, private constant values, top-level statements and ordinary comments are
replaced by handles (`⟨body:h7⟩`) the local model can answer questions about. A file that does not
parse cleanly, or is in another language, is treated as sealed. Files matched by `ip.sealed` show
existence only. Listings mark both; searches report locations only; `diff` omits their changes; the
frontier cannot write them with `edit_file` or `write_file`.

**Command access.** The project must still build and its tests must still run, and they read the
protected files. Duet therefore separates three kinds of process:

| Process | May read protected source | May read sensitive data | What the frontier gets |
|---|---|---|---|
| `run_command` (ordinary) | no (OS sandbox) | no | output, scanned |
| `run_command` with `sensitive_data` | yes | yes | a handle and the **recognised result lines** only |
| Checks (`finish`, `edit_protected`) | yes | no | the **recognised result lines** only |

Recognised result lines are a fixed set of shapes: exit status, test outcomes and summaries
(libtest, TAP, jest- and pytest-style), compiler diagnostic headers and locations, panic
locations with their message line, and assertion `left`/`right` values, each length-capped. Every
other line is withheld and stays available to `ask_local`. Before anything is sent, every line
that contains a distinctive line of protected code (a compiler snippet, a quoted body) is replaced,
string literals and distinctive tokens of protected bodies are replaced by placeholders wherever
they appear, and the copied-span filter covers protected bodies.

**Protected edits.** `edit_protected(path, spec, tests?, command?)`: the frontier describes the
change and may supply test code (written to an open file). The local model rewrites the protected
file on this machine; the host writes it through the guarded write path and runs the command as a
check. The frontier receives pass/fail, the recognised result lines and which items changed (by
name). On failure the local model gets the raw check output and one more attempt.

Limits of this design:

- Program behaviour is observable: tests the frontier writes can probe what protected code
  computes, and assertion values, panic messages and compiler messages (which name identifiers
  and types) cross. A frontier that deliberately writes code to re-encode protected source into
  those channels (for example, a failing assertion whose value is an encoded slice of the file) can
  extract it a few bytes per run. The design stops accidental disclosure by an honest-but-curious
  frontier; it does not stop a deliberately adversarial one.
- Line redaction needs a whole distinctive line (at least 16 characters and 3 words); fragments of
  a line pass unless they contain a protected literal or form a ≥24-token copied span.
- Local answers about protected bodies can paraphrase the logic.
- The skeleton itself (names, signatures, doc comments, public constants) is disclosed by design.

## Verification

The claim is measured, not assumed. The dogfood suite ([docs/DOGFOOD_SUITE.md](docs/DOGFOOD_SUITE.md))
plants unique canaries — secrets, personal data, business facts, source strings, protected function
bodies and prompt-injection text — in every run, and an independent logging proxy between Duet and
the provider records every request. A release requires zero canaries in outbound traffic.

## Known limits

- Detectors cannot recognize every possible secret format; canaries and the audit log exist to
  measure what gets through.
- The copied-span filter works at roughly 24 tokens; shorter fragments of sensitive text can pass.
- Summaries and answers written by the local model are derived from sensitive content by design.
- Files written into `target/` or `node_modules/` by a `sensitive_data` command are not tracked
  as derived data (they are build output); a program that stores derived data there escapes that rule.

## Reporting a vulnerability

Report suspected disclosure paths privately to the maintainer before publishing details.
