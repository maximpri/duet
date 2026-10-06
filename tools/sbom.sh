#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Writes a CycloneDX 1.5 JSON SBOM of the `declass` binary (package declass-cli):
# every normal and build dependency, transitively, for the host platform, with
# its license expression, crates.io archive SHA-256 and repository. Offline:
# it reads `cargo metadata --locked --offline` and Cargo.lock only; no plugin.
#
#   tools/sbom.sh [--out FILE] [--target TRIPLE] [--package NAME]
#
# SOURCE_DATE_EPOCH fixes the timestamp (tools/release.sh sets it to the commit
# time), so the same commit gives the same document.
set -euo pipefail
cd "$(dirname "$0")/.."
exec cargo run --quiet --locked --offline -p declass-release --bin declass-sbom -- "$@"
