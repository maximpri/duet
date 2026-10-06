#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Compile and run release/platform regressions on an explicit Linux architecture.
# Usage: tools/release-platform-check.sh --platform linux/amd64 --rust 1.90.0 --target-dir /absolute/cache
# Docker may emulate the architecture. The worktree is read-only; all Cargo
# downloads and build products stay in the selected external cache directory.
set -euo pipefail
cd "$(dirname "$0")/.."
platform=""
rust_version=1.90.0
target_dir=""
while [ $# -gt 0 ]; do
    case "$1" in
    --platform | --rust | --target-dir)
        [ $# -ge 2 ] || { echo "$1 requires a value" >&2; exit 2; }
        case "$1" in --platform) platform="$2" ;; --rust) rust_version="$2" ;; --target-dir) target_dir="$2" ;; esac
        shift 2 ;;
    *) echo 'usage: release-platform-check.sh --platform linux/amd64|linux/arm64 --rust VERSION --target-dir /absolute/cache' >&2; exit 2 ;;
    esac
done
case "$platform" in linux/amd64 | linux/arm64) ;; *) echo 'choose linux/amd64 or linux/arm64' >&2; exit 2 ;; esac
[[ "$rust_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo 'Rust version must be explicit x.y.z' >&2; exit 2; }
case "$target_dir" in /*) ;; *) echo '--target-dir must be an absolute path' >&2; exit 2 ;; esac
mkdir -p "$target_dir"
target_dir=$(cd "$target_dir" && pwd -P)
# Freeze the selected worktree files: edits in another terminal must not change
# a shell script halfway through execution inside a long-running platform test.
snapshot=$(mktemp -d "${target_dir}.source.XXXXXXXX")
trap 'rm -rf "$snapshot"' EXIT
git ls-files --cached --others --exclude-standard -z | while IFS= read -r -d '' name; do
    if [ -f "$name" ] || [ -L "$name" ]; then printf '%s\0' "$name"; fi
done | tar --null -T - -cf - | tar -xf - -C "$snapshot"
arch=${platform#linux/}
image="declass-release-check:$rust_version-$arch"
base="rust:$rust_version-bookworm"
docker build --platform "$platform" -t "$image" - <<DOCKER
FROM $base
RUN apt-get update && apt-get install -y --no-install-recommends bubblewrap openssh-client \\
    && rm -rf /var/lib/apt/lists/*
DOCKER
docker image inspect "$image" --format 'image={{.Id}} architecture={{.Architecture}}'
# Isolated privileged container enables bubblewrap's namespace tests. Only the
# source and caller-selected build cache are mounted; no home or Docker socket.
docker run --rm --platform "$platform" --privileged \
    --mount "type=bind,source=$snapshot,target=/src,readonly" \
    --mount "type=bind,source=$target_dir,target=/cache" \
    --env CARGO_HOME=/cache/cargo-home --env CARGO_TARGET_DIR=/cache/build \
    --env CARGO_BUILD_JOBS=2 --env DECLASS_SANDBOX_BRIDGE=/cache/build/debug/declass-sandbox-bridge \
    --workdir /src "$image" bash -eu -c '
        printf "platform: "; uname -sm
        rustc --version
        cargo --version
        bwrap --version
        sha256sum Cargo.lock
        # SBOM metadata resolves all platforms offline, not only the host
        # compiled dependencies. Populate that closure before the offline test.
        cargo fetch --locked
        DECLASS_RELEASE_BUILD=1 cargo build --release --locked -p declass-cli --bin declass
        /cache/build/release/declass --version
        /cache/build/release/declass --help >/dev/null
        cargo test --locked -p declass-release
        cargo test --locked -p declass-cli --test setup_doctor --test provider_proxy
        cargo test --locked -p declass-sandbox
    '
printf 'release-platform-check: %s Rust %s passed\n' "$platform" "$rust_version"
