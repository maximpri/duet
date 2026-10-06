#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Deploy an already-built native binary over the owner's normal SSH connection.
# Usage: tools/deploy.sh [user@]host [binary]
set -euo pipefail

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo "usage: tools/deploy.sh [user@]host [binary]" >&2
    exit 2
fi
host="$1"
case "$host" in
    -*|*[[:space:]]*|'') echo "invalid SSH destination" >&2; exit 2 ;;
esac
repo="$(cd "$(dirname "$0")/.." && pwd -P)"
binary="${2:-$repo/target/release/declass}"
if [ ! -f "$binary" ] || [ ! -x "$binary" ]; then
    echo "build first: cargo build --release --locked -p declass-cli --bin declass" >&2
    exit 1
fi
"$binary" --version
if command -v shasum >/dev/null 2>&1; then
    digest="$(shasum -a 256 "$binary" | awk '{print $1}')"
else
    digest="$(sha256sum "$binary" | awk '{print $1}')"
fi
ssh_options=(-o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=2)
local_platform="$(uname -s; uname -m)"
remote_platform="$(ssh "${ssh_options[@]}" "$host" 'uname -s; uname -m')"
if [ "$remote_platform" != "$local_platform" ]; then
    printf 'Native build cannot be deployed: local %s; remote %s. Build on the target platform.\n' \
        "$local_platform" "$remote_platform" >&2
    exit 1
fi

# Only this generated identifier and SHA-256 enter remote command strings.
# The remote home directory is expanded by its own shell, never interpolated here.
deploy_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ssh "${ssh_options[@]}" "$host" \
    "umask 077; mkdir -p \"\$HOME/.local/bin\" && mkdir \"\$HOME/.local/bin/.declass-deploy-$deploy_id\""
cleanup() {
    ssh "${ssh_options[@]}" "$host" \
        "rm -f \"\$HOME/.local/bin/.declass-deploy-$deploy_id/declass\"; rmdir \"\$HOME/.local/bin/.declass-deploy-$deploy_id\" 2>/dev/null || true" \
        >/dev/null 2>&1 || true
}
trap cleanup EXIT
ssh "${ssh_options[@]}" "$host" \
    "umask 077; set -C; cat > \"\$HOME/.local/bin/.declass-deploy-$deploy_id/declass\"" < "$binary"

ssh "${ssh_options[@]}" "$host" "sh -s -- '$deploy_id' '$digest'" <<'REMOTE'
set -eu
deploy_id="$1"
expected="$2"
bin="$HOME/.local/bin"
stage="$bin/.declass-deploy-$deploy_id"
if command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$stage/declass" | awk '{print $1}')"
else
    actual="$(sha256sum "$stage/declass" | awk '{print $1}')"
fi
if [ "$actual" != "$expected" ]; then
    echo "transfer checksum mismatch; installation stopped" >&2
    exit 1
fi
chmod 755 "$stage/declass"
# This also catches an incompatible OS version before replacing an installation.
"$stage/declass" --version
"$stage/declass" --help >/dev/null
if [ -d "$bin/declass" ]; then
    echo "$bin/declass is a directory; installation stopped" >&2
    exit 1
fi
if [ -e "$bin/declass" ]; then
    cp -p "$bin/declass" "$bin/declass.backup-$deploy_id"
    printf 'Previous binary: %s\n' "$bin/declass.backup-$deploy_id"
fi
mv -f "$stage/declass" "$bin/declass"

# Configure future interactive/login shells; preserve existing startup files.
path_line='case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) export PATH="$HOME/.local/bin:$PATH" ;; esac'
case "${SHELL##*/}" in
    zsh) set -- .zprofile .zshrc ;;
    bash)
        if [ -f "$HOME/.bash_profile" ]; then
            set -- .bash_profile .bashrc
        elif [ -f "$HOME/.bash_login" ]; then
            set -- .bash_login .bashrc
        else
            set -- .profile .bashrc
        fi
        ;;
    sh|dash|ksh) set -- .profile ;;
    *) set -- ;;
esac
for name do
    profile="$HOME/$name"
    if ! grep -Fqx "$path_line" "$profile" 2>/dev/null; then
        if [ -e "$profile" ]; then
            cp -p "$profile" "$profile.declass-backup-$deploy_id"
        fi
        printf '\n# Declass command\n%s\n' "$path_line" >> "$profile"
    fi
done
"$bin/declass" --version
printf 'Installed: %s\nSHA-256: %s\n' "$bin/declass" "$actual"
printf 'Run now: %s\nNew zsh/bash sessions can use: declass\n' "$bin/declass"
REMOTE
