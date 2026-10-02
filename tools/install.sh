#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build this checkout and install duet in the user's bin directory.
# No network is used after Cargo has its locked dependencies cached.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd -P)"
cd "$repo"

for tool in cargo rustc mktemp cp chmod mv mkdir; do
    command -v "$tool" >/dev/null 2>&1 || {
        printf 'duet install: required command not found: %s\n' "$tool" >&2
        exit 1
    }
done

install_dir="${DUET_INSTALL_DIR:-$HOME/.local/bin}"
case "$install_dir" in
    /*) ;;
    *) printf 'duet install: DUET_INSTALL_DIR must be an absolute path\n' >&2; exit 1 ;;
esac

target_dir="${CARGO_TARGET_DIR:-$repo/target}"
case "$target_dir" in
    /*) ;;
    *) target_dir="$repo/$target_dir" ;;
esac

printf 'Building duet from this checkout with Cargo.lock...\n'
cargo build --release --locked -p duet-cli --bin duet

if [ -n "${CARGO_BUILD_TARGET:-}" ]; then
    binary="$target_dir/$CARGO_BUILD_TARGET/release/duet"
else
    binary="$target_dir/release/duet"
fi
[ -f "$binary" ] || {
    printf 'duet install: built binary not found at %s\n' "$binary" >&2
    exit 1
}
"$binary" --version

mkdir -p "$install_dir"
staged="$(mktemp "$install_dir/.duet.XXXXXXXX")"
cleanup() { rm -f "$staged"; }
trap cleanup EXIT
cp "$binary" "$staged"
chmod 755 "$staged"
mv -f "$staged" "$install_dir/duet"
trap - EXIT

printf 'Installed %s/duet\n' "$install_dir"
printf 'Check your setup: %s/duet doctor\n' "$install_dir"
