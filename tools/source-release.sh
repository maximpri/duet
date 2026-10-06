#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Package the committed source and locked dependencies, retaining every
# upstream license/notice. Only tracked files at the release commit enter the source snapshot.
# Usage: tools/source-release.sh <output.tar.gz>
set -euo pipefail
# macOS tar otherwise serializes local extended attributes as AppleDouble files.
# GNU tar ignores this environment variable; source contents stay portable.
export COPYFILE_DISABLE=1
cd "$(dirname "$0")/.."

[ "$#" -eq 1 ] || { echo "usage: tools/source-release.sh <output.tar.gz>" >&2; exit 2; }
[ -z "$(git status --porcelain)" ] || {
    echo "source-release: release from a clean, committed checkout" >&2
    exit 1
}
[ ! -e "$1" ] || { echo "source-release: output already exists: $1" >&2; exit 1; }
# Resolve before entering the temporary source directory.
output="$(cd "$(dirname "$1")" && pwd -P)/$(basename "$1")"
staging=$(mktemp -d "${TMPDIR:-/tmp}/declass-source.XXXXXXXX")
trap 'rm -rf "$staging"' EXIT
mkdir "$staging/declass-source"
git archive HEAD | tar -xf - -C "$staging/declass-source"
(
    cd "$staging/declass-source"
    mkdir -p .cargo
    # Cargo preserves the complete published crate contents, including nested
    # third-party licenses (e.g. cryptographic C/assembly source notices).
    cargo vendor --locked --offline --versioned-dirs vendor >"$staging/vendor-config.toml"
    cat "$staging/vendor-config.toml" >>.cargo/config.toml
)
tar -czf "$staging/source.tar.gz" -C "$staging" declass-source
mv "$staging/source.tar.gz" "$output"
printf 'Corresponding source: %s\n' "$output"
