#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds a signed release of `duet` from a clean checkout.
#
#   tools/release.sh <version> --key <ssh signing key> [--out DIR]
#
# <version> must equal the workspace version in Cargo.toml. The key is the
# operator's SSH signing key (private key, or its public key while the private
# key is loaded in ssh-agent); DUET_RELEASE_KEY may name it instead of --key.
# No key is generated, and a key inside this repository is refused.
#
# Steps: the full gate (tools/gate.sh); `cargo build --release --locked` for the
# host target; the SBOM (tools/sbom.sh); BUILDINFO.txt (version, commit,
# toolchain); SHA256SUMS over every file; SHA256SUMS.sig, a detached SSH
# signature (`ssh-keygen -Y sign`, namespace duet-release). Output:
# dist/duet-<version>/ unless --out is given. Check with tools/verify-release.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
repo=$(pwd -P)

die() {
    echo "release: $*" >&2
    exit 1
}
usage() {
    sed -n '3,16p' "$0" | sed 's/^# \{0,1\}//'
}

version=""
key="${DUET_RELEASE_KEY:-}"
out=""
while [ $# -gt 0 ]; do
    case "$1" in
    -h | --help)
        usage
        exit 0
        ;;
    --key)
        [ $# -ge 2 ] || die "--key needs a path"
        key="$2"
        shift 2
        ;;
    --out)
        [ $# -ge 2 ] || die "--out needs a directory"
        out="$2"
        shift 2
        ;;
    -*) die "unknown option $1" ;;
    *)
        [ -z "$version" ] || die "one version only"
        version="$1"
        shift
        ;;
    esac
done

[ -n "$version" ] || {
    usage >&2
    exit 2
}
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || die "version $version is not x.y.z[-pre]"
workspace_version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)
[ "$version" = "$workspace_version" ] ||
    die "version $version does not match the workspace version $workspace_version in Cargo.toml"

# The signing key is the operator's; the script never creates one.
[ -n "$key" ] || die "a signing key is required: --key <path to your SSH signing key> (or DUET_RELEASE_KEY)"
[ -f "$key" ] && [ -r "$key" ] || die "signing key $key is not a readable file"
key_abs="$(cd "$(dirname "$key")" && pwd -P)/$(basename "$key")"
case "$key_abs" in
"$repo"/*) die "signing key $key is inside the repository; keep keys outside it" ;;
esac
command -v ssh-keygen >/dev/null || die "ssh-keygen is required for signing"

[ -z "$(git status --porcelain --untracked-files=no)" ] ||
    die "the working tree has uncommitted changes; release from a clean commit"
commit=$(git rev-parse HEAD)

out="${out:-dist/duet-$version}"
[ ! -e "$out" ] || die "$out already exists"

echo "== gate"
tools/gate.sh

target=$(rustc -vV | sed -n 's/^host: //p')
echo "== build ($target)"
# DUET_RELEASE_BUILD marks the binary as a release build (`duet doctor` then
# warns when no release signing keys are present).
DUET_RELEASE_BUILD=1 cargo build --release --locked -p duet-cli --bin duet --target "$target"
target_dir=$(cargo metadata --format-version 1 --no-deps --offline |
    sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
[ -n "$target_dir" ] || die "cannot find cargo's target directory"

export SOURCE_DATE_EPOCH
SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
mkdir -p "$out"
binary="duet-$version-$target"
cp "$target_dir/$target/release/duet" "$out/$binary"

echo "== SBOM"
tools/sbom.sh --target "$target" --out "$out/duet-$version.cdx.json"

cat >"$out/BUILDINFO.txt" <<INFO
duet $version
commit $commit
target $target
$(rustc -V)
$(cargo -V)
source_date_epoch $SOURCE_DATE_EPOCH
built with: DUET_RELEASE_BUILD=1 cargo build --release --locked -p duet-cli --bin duet --target $target
INFO

echo "== checksums and signature"
if command -v shasum >/dev/null; then
    sha256() { shasum -a 256 "$@"; }
else
    sha256() { sha256sum "$@"; }
fi
(
    cd "$out"
    sha256 "$binary" "duet-$version.cdx.json" BUILDINFO.txt >SHA256SUMS
)
ssh-keygen -Y sign -f "$key" -n duet-release "$out/SHA256SUMS"
[ -s "$out/SHA256SUMS.sig" ] || die "signing produced no signature"

echo
echo "release $version written to $out:"
ls -1 "$out"
echo
echo "signed by: $(ssh-keygen -l -f "$key" 2>/dev/null || echo "(key fingerprint unavailable)")"
echo "Publish SHA256SUMS.sig with the files, and your allowed-signers line"
echo "  <identity> namespaces=\"duet-release\" <public key>"
echo "through a channel users already trust. Verify: tools/verify-release.sh $out"
