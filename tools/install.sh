#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build this checkout and install declass in the user's bin directory.
# No network is used after Cargo has its locked dependencies cached.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd -P)"
cd "$repo"

for tool in cargo rustc mktemp cp chmod mv mkdir rm; do
    command -v "$tool" >/dev/null 2>&1 || {
        printf 'declass install: required command not found: %s\n' "$tool" >&2
        exit 1
    }
done

install_dir="${DECLASS_INSTALL_DIR:-$HOME/.local/bin}"
case "$install_dir" in
    /*) ;;
    *) printf 'declass install: DECLASS_INSTALL_DIR must be an absolute path\n' >&2; exit 1 ;;
esac
[ ! -d "$install_dir/declass" ] || {
    printf 'declass install: destination is a directory: %s/declass\n' "$install_dir" >&2
    exit 1
}

target_dir="${CARGO_TARGET_DIR:-$repo/target}"
case "$target_dir" in
    /*) ;;
    *) target_dir="$repo/$target_dir" ;;
esac

build_target="${CARGO_BUILD_TARGET:-}"
if [ -z "$build_target" ]; then
    # An installer builds for this machine even if Cargo config selects a
    # different default target. Explicit paths also override target-dir config.
    compiler_info="$(rustc -vV)"
    while read -r key value; do
        if [ "$key" = 'host:' ]; then
            build_target="$value"
            break
        fi
    done <<< "$compiler_info"
fi
[ -n "$build_target" ] || {
    printf 'declass install: could not determine the Rust host target\n' >&2
    exit 1
}

printf 'Building declass from this checkout with Cargo.lock...\n'
cargo build --release --locked -p declass-cli --bin declass \
    --target "$build_target" --target-dir "$target_dir"

# Cargo names custom JSON-target output directories after the target file.
target_name="${build_target##*/}"
target_name="${target_name%.json}"
binary="$target_dir/$target_name/release/declass"
[ -f "$binary" ] || {
    printf 'declass install: built binary not found at %s\n' "$binary" >&2
    exit 1
}
"$binary" --version

mkdir -p "$install_dir"
staged="$(mktemp "$install_dir/.declass.XXXXXXXX")"
cleanup() { rm -f "$staged"; }
trap cleanup EXIT
cp "$binary" "$staged"
chmod 755 "$staged"
mv -f "$staged" "$install_dir/declass"
trap - EXIT

printf 'Installed %s/declass\n' "$install_dir"
printf 'Check your setup: %s/declass doctor\n' "$install_dir"
