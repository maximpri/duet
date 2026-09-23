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
3. **One outbound gate**, the only code path to the frontier: known values are re-tokenized, the
   payload is re-scanned, copied spans of sensitive content (≈24+ tokens) are removed, and nothing
   is sent while any check fails.
4. **Hash-chained audit log** of every outbound request (placeholder-substituted), verifiable with
   `duet audit verify <run>`.
5. **Write-back rules**: secrets are resolved locally and only into places where secrets belong.
6. **Local data hygiene**: raw handles, transcripts and the placeholder vault are mode 0600 under
   `.duet/runs/`, deleted after the retention period or by `duet purge`.

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
