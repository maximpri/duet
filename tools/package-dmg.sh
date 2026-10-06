#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Wrap a verified macOS payload without changing its binary or signed manifest.
set -euo pipefail
export COPYFILE_DISABLE=1
umask 077
fail() { printf 'package-dmg: %s\n' "$*" >&2; exit 1; }
usage() { echo 'usage: package-dmg.sh RELEASE_DIR --out FILE.dmg [--unsigned | --key KEY --signers FILE]'; }
release="" out="" signers="" key="" mode=unsigned
unsigned_requested=false
while [ $# -gt 0 ]; do
    case "$1" in
        --out|--signers|--key) [ $# -ge 2 ] && [ -n "$2" ] || fail "$1 requires a value"; case "$1" in --out) out=$2 ;; --signers) signers=$2 ;; --key) key=$2 ;; esac; shift 2 ;;
        --unsigned) unsigned_requested=true; shift ;;
        --help|-h) usage; exit 0 ;;
        -*) fail "unknown option: $1" ;;
        *) [ -z "$release" ] || fail 'specify one release directory'; release=$1; shift ;;
    esac
done
[ -n "$release" ] && [ -n "$out" ] || { usage >&2; exit 2; }
[ "$(uname -s)" = Darwin ] || fail 'DMG packaging requires macOS'
command -v hdiutil >/dev/null || fail 'hdiutil is required'
[ -d "$release" ] && [ ! -L "$release" ] || fail 'release must be an ordinary directory'
[ ! -e "$out" ] && [ ! -L "$out" ] || fail 'output already exists'
case "$out" in *.dmg) ;; *) fail 'output must end in .dmg' ;; esac
if [ -n "$key" ]; then
    $unsigned_requested && fail '--unsigned and --key are mutually exclusive'
    [ -f "$key" ] && [ -r "$key" ] && [ -f "$signers" ] || fail 'signing needs an existing readable key and trusted --signers FILE'
    mode=signed
    key=$(cd "$(dirname "$key")" && pwd -P)/$(basename "$key")
    signers=$(cd "$(dirname "$signers")" && pwd -P)/$(basename "$signers")
else
    [ -z "$signers" ] || fail '--signers requires --key for authenticated DMG packaging'
fi
tools=$(cd "$(dirname "$0")" && pwd -P)
if [ "$mode" = signed ]; then
    repo=$(cd "$tools/.." && pwd -P)
    case "$key" in "$repo"/*) fail 'keep signing keys outside the repository' ;; esac
fi
release=$(cd "$release" && pwd -P)
mkdir -p "$(dirname "$out")"
out=$(cd "$(dirname "$out")" && pwd -P)/$(basename "$out")
for path in "$out.SHA256SUMS" "$out.SHA256SUMS.sig"; do
    [ ! -e "$path" ] && [ ! -L "$path" ] || fail 'outer verification output already exists'
done
# A sibling lock reserves the output without truncating a preexisting image.
lock="$out.lock"
mkdir "$lock" 2>/dev/null || fail 'output is already reserved'
scratch=""
cleanup() { [ -z "$scratch" ] || rm -rf "$scratch"; rmdir "$lock" 2>/dev/null || true; }
trap cleanup EXIT
scratch=$(mktemp -d "${TMPDIR:-/tmp}/declass-dmg.XXXXXXXX")
volume="$scratch/volume"
mkdir "$volume" "$volume/payload"
# Reject links/directories before copying; verification is repeated on the copy.
for file in "$release"/* "$release"/.[!.]* "$release"/..?*; do
    [ -e "$file" ] || [ -L "$file" ] || continue
    [ -f "$file" ] && [ ! -L "$file" ] || fail 'release contains a nonregular artifact'
    cp -p "$file" "$volume/payload/"
done
cp "$tools/dmg/Install Declass.command" "$volume/Install Declass.command"
cp "$tools/verify-release.sh" "$volume/verify-release.sh"
cp "$tools/verify-checksums.sh" "$volume/verify-checksums.sh"
printf '%s\n' "$mode" >"$volume/INSTALL-MODE"
chmod 755 "$volume/Install Declass.command" "$volume/verify-release.sh" "$volume/verify-checksums.sh"
if [ "$mode" = signed ]; then
    /bin/bash "$volume/Install Declass.command" --verify-only --signers "$signers"
else
    /bin/bash "$volume/Install Declass.command" --verify-only --unsigned
fi
version=$(sed -n 's/^declass //p' "$volume/payload/BUILDINFO.txt")
target=$(sed -n 's/^target //p' "$volume/payload/BUILDINFO.txt")
cat >"$volume/README.txt" <<INFO
Declass $version ($target) — CLI/TUI installer
Release verification mode: $mode

Open Install Declass.command to install to your own ~/.local/bin (no sudo).
All payload files, including full corresponding source and licenses, are retained
under ~/.local/share/declass/releases. No network access is needed by this installer.
The payload/ directory contains the original release files and checksums.

Unsigned mode requires explicit confirmation or --unsigned. Checksums do not
identify a publisher. Signed mode verifies the payload's SSH release signature
using your independently obtained ~/.config/declass/allowed_signers file (or
DECLASS_CONFIG_HOME/allowed_signers); --signers FILE selects another trusted file.
This image does not supply its own trusted keys. Before opening a signed image,
verify the separate DMG.SHA256SUMS.sig (SSH namespace declass-release) with your
independently trusted signer, then the DMG checksum. That authenticates the whole
image including this installer. SSH signatures are not Apple authentication.

No Apple code signing or notarization is performed or claimed. Apple signing
must precede creation of the payload checksums and release signature. This tool
never strips quarantine or bypasses Gatekeeper. If macOS blocks an unsigned
candidate, stop and use only an installation method approved for your machine.
See SOURCE.txt, LICENSE, NOTICE and LICENSES.md inside payload/ for distribution.
INFO
# Normalize mounted permissions independently of the packager's umask/ownership.
# Only staging modes change; signed file bytes and the original payload stay intact.
chmod 755 "$volume" "$volume/payload"
for file in "$volume/payload"/* "$volume/payload"/.[!.]* "$volume/payload"/..?*; do
    [ -f "$file" ] || continue
    chmod 644 "$file"
done
chmod 644 "$volume/README.txt" "$volume/INSTALL-MODE"
chmod 755 "$volume/payload/declass-$version-$target"
hdiutil create -quiet -srcfolder "$volume" -volname "Declass $version $mode" -fs HFS+ -format UDZO "$scratch/candidate.dmg"
# Hard-link publication is exclusive: a late competing file is never replaced.
cp "$scratch/candidate.dmg" "$lock/candidate.dmg"
ln "$lock/candidate.dmg" "$out" || fail 'output appeared while packaging'
rm "$lock/candidate.dmg"
if command -v shasum >/dev/null; then
    (cd "$(dirname "$out")" && shasum -a 256 "$(basename "$out")") >"$lock/checksums"
else
    (cd "$(dirname "$out")" && sha256sum "$(basename "$out")") >"$lock/checksums"
fi
if [ "$mode" = signed ]; then
    ssh-keygen -Y sign -f "$key" -n declass-release "$lock/checksums"
    principals=$(ssh-keygen -Y find-principals -s "$lock/checksums.sig" -f "$signers") || fail 'outer signature has no trusted signer'
    verified=false
    while IFS= read -r identity; do
        if ssh-keygen -Y verify -f "$signers" -I "$identity" -n declass-release -s "$lock/checksums.sig" <"$lock/checksums" >/dev/null 2>&1; then verified=true; break; fi
    done <<<"$principals"
    $verified || fail 'outer signature did not verify'
fi
ln "$lock/checksums" "$out.SHA256SUMS" || fail 'checksum output appeared while packaging'
rm "$lock/checksums"
if [ "$mode" = signed ]; then
    ln "$lock/checksums.sig" "$out.SHA256SUMS.sig" || fail 'signature output appeared while packaging'
    rm "$lock/checksums.sig"
fi
printf 'DMG (%s payload; no Apple authentication claimed): %s\n' "$mode" "$out"
