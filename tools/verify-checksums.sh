#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Verify strict flat SHA256SUMS coverage and file integrity; no publisher authentication.
# Usage: verify-checksums.sh DIRECTORY [--count]
set -euo pipefail

die() { printf 'checksums: FAILED: %s\n' "$*" >&2; exit 1; }
dir=""
count_only=false
while [ $# -gt 0 ]; do
    case "$1" in
        --count) count_only=true ;;
        -*) die "unknown option: $1" ;;
        *) [ -z "$dir" ] || die 'one release directory only'; dir=$1 ;;
    esac
    shift
done
[ -n "$dir" ] && [ -d "$dir" ] || die 'specify a release directory'
sums="$dir/SHA256SUMS"
[ -f "$sums" ] && [ ! -L "$sums" ] || die 'SHA256SUMS must be a regular file'
command -v shasum >/dev/null 2>&1 || command -v sha256sum >/dev/null 2>&1 || die 'shasum or sha256sum is required'

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

if $count_only; then
    printf '%s\n' "$count"
else
    printf 'verified: %s file(s) match SHA256SUMS (integrity only; publisher not authenticated)\n' "$count"
fi
