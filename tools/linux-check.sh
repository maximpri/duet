#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Runs the Linux (bubblewrap) sandbox, agent, boundary and egress tests (and their lints) in
# Docker, in four setups:
#   privileged    --privileged; tests run as a non-root user
#   unprivileged  no added capabilities, tests as a non-root user; bubblewrap
#                 uses unprivileged user namespaces (the usual Linux desktop)
#   root          --privileged, tests run as root: commands must still hold
#                 no capabilities
#   no-namespaces Docker's default profile, which forbids new namespaces:
#                 commands must be refused (fail closed), never run unsandboxed
#
#   tools/linux-check.sh            run all four, then remove everything it made
#   tools/linux-check.sh --keep     keep the image and the cargo volume (faster reruns)
#   tools/linux-check.sh --rmi-base also remove the base image if nothing else uses it
#
# Every Docker object it creates is named declass-linux-check*; nothing else is touched.
# The worktree is mounted read-only; builds go to the volume declass-linux-check-target.
# DECLASS_LINUX_TARGET_DIR=/absolute/path uses a persistent host directory instead
# (for example, a larger external disk). Cleanup never removes that directory.
set -euo pipefail
cd "$(dirname "$0")/.."
src=$(pwd)

prefix=declass-linux-check
image=$prefix:local
volume=$prefix-target
target_dir=${DECLASS_LINUX_TARGET_DIR:-}
base=${DECLASS_LINUX_BASE:-rust:1-bookworm}
crates=(-p declass-sandbox -p declass-agent -p declass-boundary -p declass-egress)
keep=false
rmi_base=false
for arg in "$@"; do
    case "$arg" in
    --keep) keep=true ;;
    --rmi-base) rmi_base=true ;;
    *)
        echo "usage: tools/linux-check.sh [--keep] [--rmi-base]" >&2
        exit 2
        ;;
    esac
done

if [ -n "$target_dir" ]; then
    case "$target_dir" in
    /*) ;;
    *) echo "DECLASS_LINUX_TARGET_DIR must be an absolute path" >&2; exit 2 ;;
    esac
    mkdir -p "$target_dir"
    target_dir=$(cd "$target_dir" && pwd -P)
fi

cleanup() {
    for mode in privileged unprivileged root no-namespaces; do
        docker rm -f "$prefix-$mode" >/dev/null 2>&1 || true
    done
    if ! $keep; then
        if [ -z "$target_dir" ]; then
            docker volume rm "$volume" >/dev/null 2>&1 || true
        fi
        docker rmi "$image" >/dev/null 2>&1 || true
        if $rmi_base && [ -z "$(docker ps -aq --filter "ancestor=$base")" ]; then
            docker rmi "$base" >/dev/null 2>&1 || true
        fi
    fi
}
trap cleanup EXIT

docker build -q -t "$image" - <<EOF >/dev/null
FROM $base
RUN apt-get update && apt-get install -y --no-install-recommends bubblewrap git \
    && rm -rf /var/lib/apt/lists/*
RUN rustup component add clippy && useradd -m -u 1000 declass
EOF
if [ -n "$target_dir" ]; then
    target_mount=(--mount "type=bind,source=$target_dir,target=/target")
else
    docker volume create "$volume" >/dev/null
    target_mount=(--mount "type=volume,source=$volume,target=/target")
fi

# Runs `cargo $*` in container $mode (as root only in the root setup).
run_mode() {
    local mode=$1
    shift
    local opts=() user=declass
    case "$mode" in
    privileged) opts=(--privileged) ;;
    root)
        opts=(--privileged)
        user=root
        ;;
    unprivileged) opts=(--security-opt seccomp=unconfined --security-opt systempaths=unconfined) ;;
    no-namespaces) opts=(-e DECLASS_EXPECT_NO_NAMESPACES=1) ;;
    esac
    printf '\n== %s\n' "$mode"
    docker run --rm --name "$prefix-$mode" "${opts[@]}" -e "DECLASS_USER=$user" \
        -v "$src:/src:ro" "${target_mount[@]}" -w /src "$image" \
        bash -c 'chown declass /target &&
            echo "kernel $(uname -r); $(bwrap --version); $(rustc --version)" &&
            exec runuser -u "$DECLASS_USER" -- env HOME=/home/declass PATH=/usr/local/cargo/bin:/usr/bin:/bin \
                RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/target/cargo-home \
                CARGO_TARGET_DIR=/target/build ${DECLASS_EXPECT_NO_NAMESPACES:+DECLASS_EXPECT_NO_NAMESPACES=1} \
                DECLASS_SANDBOX_BRIDGE=/target/build/debug/declass-sandbox-bridge \
                cargo "$@"' bash "$@"
}

status=0
run_mode privileged clippy --locked "${crates[@]}" --all-targets -- -D warnings || status=1
run_mode privileged test --locked --no-fail-fast "${crates[@]}" || status=1
run_mode unprivileged test --locked --no-fail-fast "${crates[@]}" || status=1
# The egress bridge with declass itself as the helper, through the frontier loop.
for mode in privileged unprivileged; do
    run_mode "$mode" test --locked -p declass-cli --test registry_network || status=1
done
run_mode no-namespaces test --locked -p declass-sandbox fail_closed || status=1
# Last: files it creates in the volume belong to root.
run_mode root test --locked --no-fail-fast "${crates[@]}" || status=1
if [ $status -eq 0 ]; then
    printf '\nlinux-check: all setups passed\n'
else
    printf '\nlinux-check: FAILED\n' >&2
fi
exit $status
