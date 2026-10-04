#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Offline installer for the payload beside this file. No privilege escalation.
set -euo pipefail
umask 077
fail() { printf 'Install Duet: %s\n' "$*" >&2; exit 1; }
base=$(cd "$(dirname "$0")" && pwd -P)
payload="$base/payload"
unsigned=false
verify_only=false
signers="${DUET_CONFIG_HOME:-$HOME/.config/duet}/allowed_signers"
while [ $# -gt 0 ]; do
    case "$1" in
        --unsigned) unsigned=true; shift ;;
        --verify-only) verify_only=true; shift ;;
        --signers) [ $# -ge 2 ] || fail '--signers requires a file'; signers=$2; shift 2 ;;
        *) fail 'usage: Install Duet.command [--unsigned] [--signers FILE] [--verify-only]' ;;
    esac
done
[ -f "$base/INSTALL-MODE" ] && [ ! -L "$base/INSTALL-MODE" ] || fail 'missing installation mode'
mode=$(cat "$base/INSTALL-MODE")
case "$mode" in
    signed) $unsigned && fail 'signed packages cannot bypass signature verification' ;;
    unsigned)
        if ! $unsigned; then
            [ -t 0 ] || fail 'unsigned candidate: rerun with --unsigned only if you trust its source'
            printf 'This is an unsigned, unnotarized candidate. Checksums do not authenticate its publisher.\nType INSTALL UNSIGNED to continue: '
            IFS= read -r answer
            [ "$answer" = 'INSTALL UNSIGNED' ] || fail 'installation cancelled'
        fi ;;
    *) fail 'invalid installation mode' ;;
esac

# The same strict inventory/checksum verifier is used by both installers.
verify_payload() {
    local dir=$1 name version_line target_line f
    [ -d "$dir" ] && [ ! -L "$dir" ] || fail 'payload is not an ordinary directory'
    if [ "$mode" = signed ]; then
        [ -f "$signers" ] || fail 'trusted allowed_signers required; obtain it through an independent trusted channel'
        /bin/bash "$base/verify-release.sh" "$dir" --signers "$signers" || fail 'release signature verification failed'
    else
        for f in "$dir"/*.sig; do
            [ ! -e "$f" ] && [ ! -L "$f" ] || fail 'unsigned mode refuses signature files; use the signed installation path'
        done
        /bin/bash "$base/verify-checksums.sh" "$dir" || fail 'payload checksum verification failed'
    fi
    version_line=$(sed -n 's/^duet //p' "$dir/BUILDINFO.txt")
    target_line=$(sed -n 's/^target //p' "$dir/BUILDINFO.txt")
    [[ "$version_line" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || fail 'invalid version'
    case "$target_line" in aarch64-apple-darwin|x86_64-apple-darwin) ;; *) fail 'not a macOS release' ;; esac
    for name in "duet-$version_line-$target_line" "duet-$version_line-source.tar.gz" "duet-$version_line.cdx.json" BUILDINFO.txt LICENSE NOTICE LICENSES.md LICENSE.gitleaks NOTICE.gitleaks SOURCE.txt; do
        [ -f "$dir/$name" ] && [ ! -L "$dir/$name" ] || fail "missing required artifact: $name"
    done
    version=$version_line
    target=$target_line
}
verify_payload "$payload"
if $verify_only; then printf 'Payload verified (%s): Duet %s, %s\n' "$mode" "$version" "$target"; exit 0; fi
[ "$(uname -s)" = Darwin ] || fail 'this installer requires macOS'
arch=$(uname -m)
if [ "$arch" = x86_64 ] && command -v sysctl >/dev/null 2>&1; then
    translated=$(sysctl -in sysctl.proc_translated 2>/dev/null || true)
    [ "$translated" != 1 ] || arch=arm64
fi
case "$arch:$target" in arm64:aarch64-apple-darwin|x86_64:x86_64-apple-darwin) ;; *) fail 'download the candidate matching this Mac architecture' ;; esac

# Reject links at every managed destination component. Never traverse a
# user-provided bin/release symlink or write through an existing binary link.
[ -d "$HOME" ] || fail 'home directory is unavailable'
home_dir=$(cd "$HOME" && pwd -P)
for rel in .local .local/bin .local/share .local/share/duet .local/share/duet/releases; do
    dir="$home_dir/$rel"
    [ ! -L "$dir" ] || fail "refusing linked destination: $rel"
    if [ -e "$dir" ]; then [ -d "$dir" ] && [ -O "$dir" ] || fail "destination not an owned directory: $rel"; else mkdir "$dir"; fi
done
bin="$home_dir/.local/bin"
records="$home_dir/.local/share/duet/releases"
lock="$records/.install-lock"
mkdir "$lock" 2>/dev/null || fail 'another install is active (or an interrupted install left .install-lock)'
stage=""
binary_tmp=""
cleanup() {
    [ -z "$stage" ] || rm -rf "$stage"
    [ -z "$binary_tmp" ] || rm -f "$binary_tmp"
    rmdir "$lock" 2>/dev/null || true
}
trap cleanup EXIT
stage=$(mktemp -d "$records/.stage.XXXXXXXX")
cp -p "$payload"/* "$stage/"
verify_payload "$stage"
if command -v shasum >/dev/null; then digest=$(shasum -a 256 "$stage/SHA256SUMS" | awk '{print $1}'); else digest=$(sha256sum "$stage/SHA256SUMS" | awk '{print $1}'); fi
record="$records/$version-$target-$digest"
if [ -e "$record" ] || [ -L "$record" ]; then
    verify_payload "$record"
    cmp -s "$record/SHA256SUMS" "$stage/SHA256SUMS" || fail 'existing retained manifest differs'
else
    mv "$stage" "$record"
    stage=""
fi
[ ! -L "$bin/duet" ] || fail 'refusing to replace a linked duet binary'
if [ -e "$bin/duet" ]; then [ -f "$bin/duet" ] && [ -O "$bin/duet" ] || fail 'existing duet is not an owned regular file'; fi
binary_tmp=$(mktemp "$bin/.duet-install.XXXXXXXX")
cp -p "$record/duet-$version-$target" "$binary_tmp"
chmod 755 "$binary_tmp"
cmp -s "$record/duet-$version-$target" "$binary_tmp" || fail 'installed copy differs'
actual_version=$("$binary_tmp" --version) || fail 'candidate cannot run; existing installation was preserved'
[ "$actual_version" = "duet $version" ] || fail 'candidate version differs; existing installation was preserved'
mv -f "$binary_tmp" "$bin/duet"
binary_tmp=""
printf '\nInstalled: %s/duet\nSource, licenses and verification records: %s\n' "$bin" "$record"
printf 'Run: "%s/duet"\nTo use duet by name, add ~/.local/bin to your shell PATH.\n' "$bin"
printf 'Mode: %s. No Apple signing or notarization is asserted; quarantine was not removed.\n' "$mode"
