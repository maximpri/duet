#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Verifies a release directory made by tools/release.sh: SHA256SUMS carries a
# valid SSH signature (namespace duet-release) by a key in the allowed-signers
# file, and every file in the directory is listed there and matches.
#
#   tools/verify-release.sh <release dir> [--signers FILE]
#
# The signers file defaults to allowed_signers next to the owner config
# ($DUET_CONFIG_HOME/allowed_signers, else ~/.config/duet/allowed_signers),
# the file `duet doctor` checks. Each line: <identity> namespaces="duet-release" <public key>.
# Offline; needs ssh-keygen and shasum (or sha256sum). Exit 0 only when all holds.
set -euo pipefail

die() {
    echo "verify: FAILED: $*" >&2
    exit 1
}

dir=""
signers="${DUET_CONFIG_HOME:-$HOME/.config/duet}/allowed_signers"
while [ $# -gt 0 ]; do
    case "$1" in
    -h | --help)
        sed -n '3,12p' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    --signers)
        [ $# -ge 2 ] || die "--signers needs a file"
        signers="$2"
        shift 2
        ;;
    -*) die "unknown option $1" ;;
    *)
        [ -z "$dir" ] || die "one release directory only"
        dir="$1"
        shift
        ;;
    esac
done
[ -n "$dir" ] || {
    echo "usage: tools/verify-release.sh <release dir> [--signers FILE]" >&2
    exit 2
}
[ -d "$dir" ] || die "$dir is not a directory"
[ -f "$signers" ] || die "no allowed-signers file at $signers (get the release signer line from a trusted channel)"
sums="$dir/SHA256SUMS"
sig="$dir/SHA256SUMS.sig"
[ -f "$sums" ] || die "$sums is missing"
[ -f "$sig" ] || die "$sig is missing: the release is unsigned"
command -v ssh-keygen >/dev/null || die "ssh-keygen is required"

# 1. The signature: which trusted identity made it, then a full verification.
principals=$(ssh-keygen -Y find-principals -s "$sig" -f "$signers" 2>/dev/null) ||
    die "SHA256SUMS is not signed by any key in $signers"
signer=""
while IFS= read -r p; do
    if ssh-keygen -Y verify -f "$signers" -I "$p" -n duet-release -s "$sig" <"$sums" >/dev/null 2>&1; then
        signer="$p"
        break
    fi
done <<<"$principals"
[ -n "$signer" ] || die "the signature over SHA256SUMS does not verify (namespace duet-release)"

# 2. The checksums: every line must describe exactly one ordinary filename.
# Hash tools can warn about malformed lines and still exit successfully; those
# lines must never count as covering an otherwise unchecked release file.
# Check raw bytes first: some Bash versions discard NUL bytes during `read`.
if LC_ALL=C grep -aq '[[:cntrl:]]' "$sums"; then
    die "SHA256SUMS contains a control character"
fi
sum_record='^[0-9a-fA-F]{64} [ *](.+)$'
listed=""
count=0
while IFS= read -r line || [ -n "$line" ]; do
    [[ "$line" =~ $sum_record ]] || die "SHA256SUMS contains a malformed checksum record"
    name="${BASH_REMATCH[1]}"
    case "$name" in
    "" | */* | .* | *\\*) die "SHA256SUMS lists an unexpected name: $name" ;;
    esac
    if grep -qxF -- "$name" <<<"$listed"; then
        die "SHA256SUMS lists a duplicate filename: $name"
    fi
    listed="${listed}${listed:+$'\n'}$name"
    count=$((count + 1))
done <"$sums"
[ "$count" -gt 0 ] || die "SHA256SUMS contains no files"
for f in "$dir"/* "$dir"/.[!.]* "$dir"/..?*; do
    [ -e "$f" ] || [ -L "$f" ] || continue
    [ -f "$f" ] && [ ! -L "$f" ] || die "$f is not a regular release file"
    name=$(basename "$f")
    case "$name" in SHA256SUMS | SHA256SUMS.sig) continue ;; esac
    grep -qxF -- "$name" <<<"$listed" || die "$name is in the release directory but not in SHA256SUMS"
done
if command -v shasum >/dev/null; then
    check() { shasum -a 256 -c --quiet "$@"; }
else
    check() { sha256sum -c --quiet "$@"; }
fi
(cd "$dir" && check SHA256SUMS) || die "a file does not match SHA256SUMS"

echo "verified: SHA256SUMS signed by $signer; $count file(s) match"
