#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Runs the libFuzzer targets in fuzz/ for a time budget each. Not part of the
# gate (the workspace stays on stable); run it before a release or after
# changing a parser, the vault, the copied-span filter or the detectors.
#
#   tools/fuzz.sh [SECONDS] [TARGET...]     default: 60 seconds, every target
#
# With a nightly toolchain and cargo-fuzz it uses `cargo +nightly fuzz run`.
# Without them it builds the targets on the stable toolchain with the same
# coverage instrumentation and no sanitizer (Declass forbids unsafe code, so the
# findings that matter are panics and failed assertions, which need none).
# Corpora grow in fuzz/corpus/<target>; crashing inputs land in
# fuzz/artifacts/<target>/. Reproduce one by passing it to the target binary
# (stable) or `cargo +nightly fuzz run <target> <file>`.
set -euo pipefail
cd "$(dirname "$0")/.."

case "${1:-}" in
-h | --help)
    sed -n '3,15p' "$0" | sed 's/^# \{0,1\}//'
    cat <<'EOF'

To use cargo-fuzz (optional):
  rustup toolchain install nightly
  cargo install cargo-fuzz
EOF
    exit 0
    ;;
esac

seconds="${1:-60}"
shift || true
if ! [[ "$seconds" =~ ^[0-9]+$ ]]; then
    echo "usage: tools/fuzz.sh [SECONDS] [TARGET...]" >&2
    exit 2
fi
all_targets=$(sed -n 's/^name = "\(.*\)"$/\1/p' fuzz/Cargo.toml | grep -v '^declass-fuzz$')
targets="${*:-$all_targets}"

if cargo +nightly fuzz --version >/dev/null 2>&1; then
    for t in $targets; do
        printf '\n== fuzz %s (%ss, cargo-fuzz)\n' "$t" "$seconds"
        cargo +nightly fuzz run "$t" -- -max_total_time="$seconds"
    done
    exit 0
fi

echo "cargo +nightly fuzz is not available; building on stable without a sanitizer." >&2
echo "(For cargo-fuzz: rustup toolchain install nightly && cargo install cargo-fuzz)" >&2
host=$(rustc -vV | sed -n 's/^host: //p')
target_dir="${CARGO_TARGET_DIR:-fuzz/target}"
# --target keeps these flags off build scripts, as cargo-fuzz does.
RUSTFLAGS="--cfg fuzzing -Cpasses=sancov-module \
-Cllvm-args=-sanitizer-coverage-level=4 \
-Cllvm-args=-sanitizer-coverage-inline-8bit-counters \
-Cllvm-args=-sanitizer-coverage-pc-table \
-Cllvm-args=-sanitizer-coverage-trace-compares \
-Cdebug-assertions -Coverflow-checks" \
    cargo build --manifest-path fuzz/Cargo.toml --release --target "$host" \
    --target-dir "$target_dir" $(for t in $targets; do printf -- '--bin %s ' "$t"; done)

failed=""
for t in $targets; do
    printf '\n== fuzz %s (%ss, stable)\n' "$t" "$seconds"
    mkdir -p "fuzz/corpus/$t" "fuzz/artifacts/$t"
    if ! "$target_dir/$host/release/$t" "fuzz/corpus/$t" \
        -artifact_prefix="fuzz/artifacts/$t/" -max_total_time="$seconds" -print_final_stats=1; then
        failed="$failed $t"
    fi
done
if [ -n "$failed" ]; then
    echo "fuzz: failures in:$failed (inputs in fuzz/artifacts/)" >&2
    exit 1
fi
printf '\nfuzz: no failures\n'
