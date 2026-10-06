#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Wrap an existing signed release directory as uniquely named GitHub assets.
# Usage: tools/package-release.sh RELEASE_DIR --key KEY --signers FILE --out DIR
# Uses the operator's existing SSH signing key; never creates a production key.
set -euo pipefail
# macOS tar otherwise serializes local extended attributes as AppleDouble files.
# GNU tar ignores this environment variable; source contents stay portable.
export COPYFILE_DISABLE=1
umask 077
fail() { printf 'package-release: %s\n' "$*" >&2; exit 1; }
release_dir=""
key=""
signers=""
out=""
while [ $# -gt 0 ]; do
    case "$1" in
    --key | --signers | --out)
        [ $# -ge 2 ] || fail "$1 requires a value"
        case "$1" in --key) key="$2" ;; --signers) signers="$2" ;; --out) out="$2" ;; esac
        shift 2 ;;
    -h | --help) sed -n '3,5p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) fail "unknown option: $1" ;;
    *) [ -z "$release_dir" ] || fail 'specify one release directory'; release_dir="$1"; shift ;;
    esac
done
[ -n "$release_dir" ] && [ -n "$key" ] && [ -n "$signers" ] && [ -n "$out" ] || fail 'usage: package-release.sh RELEASE_DIR --key KEY --signers FILE --out DIR'
[ -f "$key" ] && [ -r "$key" ] || fail 'signing key is not readable'
[ ! -e "$out" ] || fail 'output directory already exists'
tools_dir="$(cd "$(dirname "$0")" && pwd -P)"
repo="$(cd "$tools_dir/.." && pwd -P)"
key_abs="$(cd "$(dirname "$key")" && pwd -P)/$(basename "$key")"
case "$key_abs" in "$repo"/*) fail 'keep signing keys outside the repository' ;; esac
"$tools_dir/verify-release.sh" "$release_dir" --signers "$signers"
version=$(sed -n 's/^declass //p' "$release_dir/BUILDINFO.txt")
target=$(sed -n 's/^target //p' "$release_dir/BUILDINFO.txt")
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || fail 'invalid signed release version'
case "$target" in aarch64-apple-darwin | x86_64-apple-darwin | aarch64-unknown-linux-gnu | x86_64-unknown-linux-gnu) ;; *) fail 'unsupported signed release target' ;; esac
stem="declass-$version-$target"
archive="$stem.tar.gz"
manifest="$stem.SHA256SUMS"
mkdir -p "$out"
# Names are explicit and flat. Installers extract only these names to stdout;
# archive-supplied paths, links and permissions never reach their filesystem.
tar -czf "$out/$archive" -C "$release_dir" "$stem" "declass-$version.cdx.json" \
    BUILDINFO.txt LICENSE NOTICE LICENSES.md LICENSE.gitleaks NOTICE.gitleaks \
    "declass-$version-source.tar.gz" SOURCE.txt SHA256SUMS SHA256SUMS.sig
if command -v shasum >/dev/null; then
    (cd "$out" && shasum -a 256 "$archive" >"$manifest")
else
    (cd "$out" && sha256sum "$archive" >"$manifest")
fi
ssh-keygen -Y sign -f "$key" -n declass-release "$out/$manifest"
printf 'GitHub release assets: %s/%s{.tar.gz,.SHA256SUMS,.SHA256SUMS.sig}\n' "$out" "$stem"
