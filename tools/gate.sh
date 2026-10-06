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

step "standalone installer source agreement"
python3 tools/build-bootstrap.py --check

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
if grep -q "declass-provider" crates/declass-agent/Cargo.toml; then
    echo "declass-agent must not depend on declass-provider (use the gate)" >&2
    exit 1
fi

step "egress by construction"
# Three places in the product open connections or resolve names (SECURITY.md,
# Egress): declass-net, the one client for third parties (web pages, search
# sources, MCP servers over HTTP), which sends only requests the boundary
# checked; declass-provider, the frontier and local-model client, which the agent
# reaches only through the outbound gate (see above); and the egress proxy's
# upstream connection in declass-egress (commands' listed registries). Allowlisted
# as well: declass-evals, the evaluation harness (it drives declass as a black box and
# runs its own leak proxy), and test code (tests/, examples/, benches/, test
# support modules). Anything else must go through declass-net.
net='\breqwest::|\buse reqwest\b|\bhyper(_util)?::|\bureq::|\bisahc::|\bsurf::|\battohttpc::|TcpStream::connect|\bUdpSocket\b|\blookup_host\b|\bto_socket_addrs\b|\bsocket2::|getaddrinfo'
hits=$( { grep -rnE "$net" crates --include='*.rs' \
            | grep -vE '^crates/[^/]+/(tests|examples|benches)/' \
            | grep -vE '^crates/(declass-net|declass-provider/src|declass-evals)/' \
            | grep -vE '^crates/declass-egress/src/lib\.rs:' \
            | grep -vE '/src/(tests|mock|mock_http)\.rs:';
          grep -nE '^(reqwest|hyper|hyper-util|ureq|isahc|surf|attohttpc|socket2|trust-dns-resolver|hickory-resolver)\b' crates/*/Cargo.toml \
            | grep -vE '^crates/(declass-net|declass-provider|declass-evals)/Cargo\.toml:'; } || true)
if [ -n "$hits" ]; then
    echo "networking outside declass-net, declass-provider and the egress proxy:" >&2
    echo "$hits" >&2
    exit 1
fi

step "model HTTP policy by construction"
# Every model/discovery/probe/catalog adapter must use the private client
# factory. It requires an ApprovedEndpoint and fixes proxies/redirects before
# returning a Client (never a loosenable builder).
model_http='(Client|ClientBuilder)::(builder|new|default)|reqwest::(get|blocking)|\.(proxy|no_proxy|redirect)\('
hits=$( { grep -rnE "$model_http" crates/declass-provider/src --include='*.rs' \
    | grep -vE '/(http|tests|mock_http)\.rs:'; } || true)
if [ -n "$hits" ]; then
    echo "model HTTP construction/policy outside the approved factory:" >&2
    echo "$hits" >&2
    exit 1
fi

step "provenance"
# Declass's implementation is independent. Product names are permitted in the
# evaluation adapters, credential denylist, and portable-skill discovery
# adapter (the user requested interoperability with standard agent files).
# Distinctive names match case-insensitively; names that are also ordinary words
# (Cursor, Goose) match only as capitalized product names.
# The clipboard decoder uses Rust's standard-library byte cursor; its import
# and constructor calls are type references, not product provenance.
pattern='(\bcodex\b|claude[ -]code|\bopencode\b|\bpi-mono\b|\baider\b|openhands|swe-agent|\bcline\b|\*\*\* Begin Patch)'
products='\b(Cursor|Goose)\b'
hits=$( { grep -rniE "$pattern" crates fuzz/fuzz_targets fuzz/Cargo.toml --include='*.rs' --include='*.toml';
          grep -rnE "$products" crates fuzz/fuzz_targets fuzz/Cargo.toml --include='*.rs' --include='*.toml'; } \
    | grep -v '^crates/declass-evals/src/lanes/' \
    | grep -v '^crates/declass-cli/src/extensions.rs:' \
    | grep -vE '^crates/declass-tui/src/clipboard\.rs:[0-9]+:.*(use std::io::\{Cursor, Read, Write\};|Cursor::new\()' \
    | grep -v '^crates/declass-sandbox/src/home_secrets.rs:' || true)
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
