# Contributing to Duet Core

Thank you for your interest. Duet Core is open source under the GNU General Public License v3.0 or
later.

## Contributor licence agreement

The copyright holder also licenses Duet Core under separate commercial terms for Duet Enterprise
(dual licensing). That is only possible while the copyright holder can license all of Duet Core's
code, so **every contribution must be made under a contributor licence agreement (CLA)** that grants
the copyright holder the right to relicense it. Pull requests without a signed CLA cannot be merged.

> The CLA text is being prepared with legal counsel and will be linked here before the repository
> is published. Until then, no outside contributions can be accepted.

## Rules for code

- **Rust only**, and **novel**: no code, prompts or designs copied from other coding agents. The gate's
  provenance check enforces part of this.
- Every commit passes `tools/gate.sh` (formatting, lints, tests, licence headers, privacy and
  provenance checks). Install the hooks after cloning: `tools/install-hooks.sh --pre-commit`.
- Every new file carries the `SPDX-License-Identifier: GPL-3.0-or-later` header.
- Dependencies must use licences allowed by `deny.toml` (permissive, GPL-compatible, and suitable
  for dual licensing: no GPL-only third-party crates).
- Security-relevant changes follow [SECURITY.md](SECURITY.md): a class-level fix, a regression test
  and, for a disclosure path, an advisory.

## Reporting a vulnerability

Do not open a public issue; follow [SECURITY.md](SECURITY.md) (Reporting a vulnerability).
