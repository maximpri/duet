#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Installs git hooks that run the gate (Declass has no hosted CI):
#   pre-push    tools/gate.sh          (full gate; always installed)
#   pre-commit  tools/gate.sh --fast   (format, license, privacy, provenance;
#                                       only with --pre-commit)
#
#   tools/install-hooks.sh [--pre-commit] [--force]
#   tools/install-hooks.sh --uninstall
#
# A hook that this script did not write is left alone unless --force is given.
# Skip the hooks for one command with `git push --no-verify` / `git commit --no-verify`.
set -euo pipefail
cd "$(dirname "$0")/.."

marker="# installed by tools/install-hooks.sh"
pre_commit=false
force=false
uninstall=false
for arg in "$@"; do
    case "$arg" in
    --pre-commit) pre_commit=true ;;
    --force) force=true ;;
    --uninstall) uninstall=true ;;
    -h | --help)
        sed -n '3,12p' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    *)
        echo "unknown option: $arg (see --help)" >&2
        exit 2
        ;;
    esac
done

# Honour core.hooksPath; otherwise the repository's hooks directory (shared by worktrees).
hooks=$(git config core.hooksPath || git rev-parse --git-path hooks)
mkdir -p "$hooks"

ours() { [ -f "$1" ] && grep -qF "$marker" "$1"; }

install() {
    local name="$1" args="$2" path="$hooks/$1"
    if [ -e "$path" ] && ! ours "$path" && ! $force; then
        echo "$path exists and was not written by this script; use --force to replace it" >&2
        return 1
    fi
    cat >"$path" <<EOF
#!/bin/sh
$marker
# Runs the gate before $name; bypass once with --no-verify.
exec "\$(git rev-parse --show-toplevel)/tools/gate.sh"$args
EOF
    chmod +x "$path"
    echo "installed $path"
}

if $uninstall; then
    for name in pre-push pre-commit; do
        if ours "$hooks/$name"; then
            rm "$hooks/$name"
            echo "removed $hooks/$name"
        fi
    done
    exit 0
fi

install pre-push ""
if $pre_commit; then
    install pre-commit " --fast"
fi
