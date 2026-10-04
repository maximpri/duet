# Contributing to Duet Core

Thank you for your interest. Duet Core is open source under the GNU General Public License v3.0 or
later.

## Contributor licence agreement

The copyright holder also licenses Duet Core under separate commercial terms for Duet Enterprise
(dual licensing). That is only possible while the copyright holder can license all of Duet Core's
code, so **every contribution must be made under a contributor licence agreement (CLA)** that grants
the copyright holder the right to relicense it. Pull requests without a signed CLA cannot be merged.

> The CLA has not been published, so outside code contributions are currently closed.
> A CLA will be linked here before we begin accepting outside code contributions.
> You can use, study, modify and redistribute Duet under GPL-3.0-or-later.

## Rules for code

- **Rust only**, and **novel**: no code, prompts or designs copied from other coding agents. The gate's
  provenance check enforces part of this.
- Every commit passes `tools/gate.sh` (formatting, lints, tests, licence headers, privacy and
  provenance checks). Install the hooks after cloning: `tools/install-hooks.sh --pre-commit`.
- New original source files carry `SPDX-License-Identifier: GPL-3.0-or-later`.
  Preserve third-party copyright/license notices; do not relabel upstream code.
- Dependencies must use licences allowed by `deny.toml` (GPL-compatible and suitable
  for dual licensing: no GPL-only third-party crates).
- Security-relevant changes follow [SECURITY.md](SECURITY.md): a class-level fix, a regression test
  and, for a disclosure path, an advisory.

## Reporting a vulnerability

Do not open a public issue; follow [SECURITY.md](SECURITY.md) (Reporting a vulnerability).

For notices, third-party material and binary distribution, see [LICENSES.md](LICENSES.md).
