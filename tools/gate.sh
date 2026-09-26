#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# The project's only gate (no hosted CI). Every step must pass.
#   tools/gate.sh          everything (the pre-push hook runs this)
#   tools/gate.sh --fast   format, license, privacy and provenance only (seconds;
#                          the optional pre-commit hook runs this)
# Property tests run with a small case count; PROPTEST_CASES raises it.
set -euo pipefail
cd "$(dirname "$0")/.."

fast=false
case "${1:-}" in
--fast) fast=true ;;
"") ;;
*)
    echo "usage: tools/gate.sh [--fast]" >&2
    exit 2
    ;;
esac

step() { printf '\n== %s\n' "$1"; }

step "format"
cargo fmt --all -- --check
rustfmt --check --edition 2024 fuzz/fuzz_targets/*.rs

if ! $fast; then
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
fi

step "license headers"
missing=$(find crates fuzz/fuzz_targets -name '*.rs' -print0 | xargs -0 grep -L 'SPDX-License-Identifier: GPL-3.0-or-later' || true)
if [ -n "$missing" ]; then
    echo "missing SPDX header:" >&2
    echo "$missing" >&2
    exit 1
fi

step "privacy by construction"
# The agent must never be able to construct an ungated provider.
if grep -q "duet-provider" crates/duet-agent/Cargo.toml; then
    echo "duet-agent must not depend on duet-provider (use the gate)" >&2
    exit 1
fi

step "provenance"
# Duet contains no code, formats or names from other coding agents. The only
# allowlisted places are the evaluation lane adapters, which launch them as
# black boxes for measurement, and the sandbox's list of home-directory
# credential stores, which names their login directories only to deny them.
# Distinctive names match case-insensitively; names that are also ordinary words
# (Cursor, Goose) match only as capitalized product names.
pattern='(\bcodex\b|claude[ -]code|\bopencode\b|\bpi-mono\b|\baider\b|openhands|swe-agent|\bcline\b|\*\*\* Begin Patch)'
products='\b(Cursor|Goose)\b'
hits=$( { grep -rniE "$pattern" crates fuzz/fuzz_targets fuzz/Cargo.toml --include='*.rs' --include='*.toml';
          grep -rnE "$products" crates fuzz/fuzz_targets fuzz/Cargo.toml --include='*.rs' --include='*.toml'; } \
    | grep -v '^crates/duet-evals/src/lanes/' \
    | grep -v '^crates/duet-sandbox/src/home_secrets.rs:' || true)
if [ -n "$hits" ]; then
    echo "provenance check failed:" >&2
    echo "$hits" >&2
    exit 1
fi

if $fast; then
    printf '\ngate (fast): checks passed; the full gate runs before push\n'
else
    printf '\ngate: all checks passed\n'
fi
