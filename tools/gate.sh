#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# The project's only gate (no hosted CI). Every step must pass.
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n== %s\n' "$1"; }

step "format"
cargo fmt --all -- --check

step "lints"
cargo clippy --workspace --all-targets --locked -- -D warnings

step "tests"
cargo test --workspace --locked

step "dependency policy"
if command -v cargo-deny >/dev/null 2>&1 || [ -x "$HOME/.cargo/bin/cargo-deny" ]; then
    PATH="$HOME/.cargo/bin:$PATH" cargo deny check
else
    echo "cargo-deny is required: cargo install cargo-deny" >&2
    exit 1
fi

step "license headers"
missing=$(find crates -name '*.rs' -print0 | xargs -0 grep -L 'SPDX-License-Identifier: GPL-3.0-or-later' || true)
if [ -n "$missing" ]; then
    echo "missing SPDX header:" >&2
    echo "$missing" >&2
    exit 1
fi

step "provenance"
# Duet contains no code, formats or names from other coding agents. The only
# allowlisted place is the evaluation lane adapters, which launch them as
# black boxes for measurement.
# Distinctive names match case-insensitively; names that are also ordinary words
# (Cursor, Goose) match only as capitalized product names.
pattern='(\bcodex\b|claude[ -]code|\bopencode\b|\bpi-mono\b|\baider\b|openhands|swe-agent|\bcline\b|\*\*\* Begin Patch)'
products='\b(Cursor|Goose)\b'
hits=$( { grep -rniE "$pattern" crates --include='*.rs' --include='*.toml';
          grep -rnE "$products" crates --include='*.rs' --include='*.toml'; } \
    | grep -v '^crates/duet-evals/src/lanes/' || true)
if [ -n "$hits" ]; then
    echo "provenance check failed:" >&2
    echo "$hits" >&2
    exit 1
fi

printf '\ngate: all checks passed\n'
