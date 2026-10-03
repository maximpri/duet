#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Install a signed release using an independently obtained allowed-signers file.
# Usage: tools/install-release.sh VERSION --signers FILE [--url HTTPS_ASSET_DIRECTORY]
# DUET_INSTALL_DIR defaults to ~/.local/bin; DUET_DATA_DIR to ~/.local/share/duet.
# Defaults to GitHub Releases (maximpri/duet, tag vVERSION). The corresponding
# source, licenses and signed release records are retained locally.
set -euo pipefail
umask 077

fail() { printf 'duet install-release: %s\n' "$*" >&2; exit 1; }
usage() { sed -n '3,7p' "$0" | sed 's/^# \{0,1\}//'; }
version=""
url=""
signers=""
while [ $# -gt 0 ]; do
    case "$1" in
    -h | --help) usage; exit 0 ;;
    --url | --signers)
        [ $# -ge 2 ] || fail "$1 requires a value"
        case "$1" in --url) url="$2" ;; --signers) signers="$2" ;; esac
        shift 2
        ;;
    -*) fail "unknown option: $1" ;;
    *) [ -z "$version" ] || fail "specify one version"; version="$1"; shift ;;
    esac
done
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || fail "specify a version as x.y.z[-pre]"
[ -n "$url" ] || url="https://github.com/maximpri/duet/releases/download/v$version"
[ -n "$signers" ] || fail "--signers is required; obtain the release signer through an independent trusted channel"
[[ "$url" =~ ^https://[^/?#]+(/[^?#]*)?$ ]] || fail "--url must be an HTTPS release-directory URL without query or fragment"
url=${url%/}
[ -f "$signers" ] && [ -r "$signers" ] || fail "no readable allowed-signers file at $signers; obtain the signer through an independent trusted channel"
for tool in curl ssh-keygen uname mktemp cp chmod mv mkdir rm grep tar; do
    command -v "$tool" >/dev/null 2>&1 || fail "required command not found: $tool"
done
verifier="$(cd "$(dirname "$0")" && pwd -P)/verify-release.sh"
[ -x "$verifier" ] || fail "verify-release.sh must be present beside this trusted installer"
case "$(uname -s):$(uname -m)" in
Darwin:arm64 | Darwin:aarch64) target=aarch64-apple-darwin ;;
Darwin:x86_64) target=x86_64-apple-darwin ;;
Linux:aarch64 | Linux:arm64) target=aarch64-unknown-linux-gnu ;;
Linux:x86_64 | Linux:amd64) target=x86_64-unknown-linux-gnu ;;
*) fail "supported platforms are macOS and GNU/Linux on ARM64 or x86-64" ;;
esac
install_dir="${DUET_INSTALL_DIR:-$HOME/.local/bin}"
data_dir="${DUET_DATA_DIR:-$HOME/.local/share/duet}"
for destination in "$install_dir" "$data_dir"; do
    case "$destination" in /*) ;; *) fail "DUET_INSTALL_DIR and DUET_DATA_DIR must be absolute paths" ;; esac
done
[ ! -d "$install_dir/duet" ] || fail "destination is a directory: $install_dir/duet"

scratch=$(mktemp -d "${TMPDIR:-/tmp}/duet-install-release.XXXXXXXX")
staged_binary=""
retained=""
cleanup() {
    rm -rf "$scratch"
    [ -z "$staged_binary" ] || rm -f "$staged_binary"
    [ -z "$retained" ] || rm -rf "$retained"
}
trap cleanup EXIT
binary="duet-$version-$target"
archive="$binary.tar.gz"
manifest="$binary.SHA256SUMS"
download() {
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 --fail --silent --show-error \
        --location --max-redirs 5 --connect-timeout 20 --max-time 600 \
        --output "$2" "$url/$1" || fail "download failed: $1"
}
# The signing key is supplied independently, never downloaded with the payload.
download "$manifest" "$scratch/SHA256SUMS"
download "$manifest.sig" "$scratch/SHA256SUMS.sig"
download "$archive" "$scratch/$archive"
"$verifier" "$scratch" --signers "$signers"
# Extract only fixed flat names to stdout into private destinations. Do not
# restore archive paths, symlinks, hardlinks or ownership, even from signed tar.
payload="$scratch/payload"
mkdir "$payload"
for name in "$binary" "duet-$version.cdx.json" BUILDINFO.txt LICENSE NOTICE LICENSES.md \
    LICENSE.gitleaks NOTICE.gitleaks "duet-$version-source.tar.gz" SOURCE.txt SHA256SUMS SHA256SUMS.sig; do
    tar -xOf "$scratch/$archive" "$name" >"$payload/$name" || fail "release archive lacks expected file: $name"
done
"$verifier" "$payload" --signers "$signers"
grep -qxF -- "duet $version" "$payload/BUILDINFO.txt" || fail "signed build metadata does not match version $version"
grep -qxF -- "target $target" "$payload/BUILDINFO.txt" || fail "signed build metadata does not match target $target"

mkdir -p "$install_dir"
staged_binary=$(mktemp "$install_dir/.duet.XXXXXXXX")
cp "$payload/$binary" "$staged_binary"
chmod 755 "$staged_binary"
# The signature and all hashes have passed before any downloaded code runs.
reported_version=$("$staged_binary" --version) || fail "verified binary cannot run on this host; existing installation retained"
[ "$reported_version" = "duet $version" ] || fail "verified binary reports an unexpected version: $reported_version"
mkdir -p "$data_dir/releases"
retained=$(mktemp -d "$data_dir/releases/$version-$target.XXXXXXXX")
cp "$payload"/* "$retained/"
# Same-directory rename is atomic: download/verification/copy failures above
# never replace the existing executable. Existing release records are retained.
mv -f "$staged_binary" "$install_dir/duet"
staged_binary=""
installed_records="$retained"
retained=""
printf 'Installed %s at %s/duet\n' "$reported_version" "$install_dir"
printf 'Verified source, licenses and release records: %s\n' "$installed_records"
